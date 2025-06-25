#!/usr/bin/env python3
# -*- coding: utf-8 -*-

import os.path
import parted
import subprocess
import json
import traceback
import time
from named_thread import NamedThread

from notifications import NotificationManager, Notification, NotificationResponse

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)


# Bootloader
PARTITIONS = [
    {
        "name": "uboot",
        "aligned": False,
        "start": 0x000020,  # in KiB
        "end": 0x002020,  # in KiB
        "fs": "fat32",
    },
    {
        "name": "uboot_env",
        "aligned": False,
        "start": 0x002020,
        "end": 0x004020,
        "fs": "fat32",
    },
    {
        "name": "root_a",
        "aligned": True,
        "start": 0x004400,
        "end": 0x504400,
        "fs": "ext4",
        "bootable": True,
    },
    {
        "name": "root_b",
        "aligned": True,
        "start": 0x504400,
        "end": 0xA04400,
        "fs": "ext4",
        "bootable": True,
    },
    # User partition fills the rest of the image
    {
        "name": "user",
        "aligned": True,
        "start": 0xA04400,
        "end": 0xA04400 + 0x2000,
        "fs": "ext4",
    },
]

PROVISION_DIR = "/meticulous-user/provisioning/"
BOOTLOADER_IMAGE = os.path.join(PROVISION_DIR, "imx-boot-sd.bin")
BOOTLOADER_SCRIPT = os.path.join(PROVISION_DIR, "u-boot.scr")
ROOTFS_ARCHIVE = os.path.join(PROVISION_DIR, "meticulous-rootfs.tar")

NOTIFICATION_IMAGE = "data:image/jpeg;base64,/9j/4AAQSkZJRgABAQAASABIAAD/4QCMRXhpZgAATU0AKgAAAAgABQEGAAMAAAABAAIAAAEaAAUAAAABAAAASgEbAAUAAAABAAAAUgEoAAMAAAABAAIAAIdpAAQAAAABAAAAWgAAAAAAAABIAAAAAQAAAEgAAAABAAOgAQADAAAAAQABAACgAgAEAAAAAQAAAHigAwAEAAAAAQAAAHgAAAAA/+EKHmh0dHA6Ly9ucy5hZG9iZS5jb20veGFwLzEuMC8APD94cGFja2V0IGJlZ2luPSLvu78iIGlkPSJXNU0wTXBDZWhpSHpyZVN6TlRjemtjOWQiPz4gPHg6eG1wbWV0YSB4bWxuczp4PSJhZG9iZTpuczptZXRhLyIgeDp4bXB0az0iWE1QIENvcmUgNi4wLjAiPiA8cmRmOlJERiB4bWxuczpyZGY9Imh0dHA6Ly93d3cudzMub3JnLzE5OTkvMDIvMjItcmRmLXN5bnRheC1ucyMiPiA8cmRmOkRlc2NyaXB0aW9uIHJkZjphYm91dD0iIiB4bWxuczpJcHRjNHhtcEV4dD0iaHR0cDovL2lwdGMub3JnL3N0ZC9JcHRjNHhtcEV4dC8yMDA4LTAyLTI5LyIgeG1sbnM6ZGM9Imh0dHA6Ly9wdXJsLm9yZy9kYy9lbGVtZW50cy8xLjEvIiBJcHRjNHhtcEV4dDpBcnR3b3JrVGl0bGU9IklNR18wMTUzIj4gPGRjOnRpdGxlPiA8cmRmOkFsdD4gPHJkZjpsaSB4bWw6bGFuZz0ieC1kZWZhdWx0Ij5JTUdfMDE1MzwvcmRmOmxpPiA8L3JkZjpBbHQ+IDwvZGM6dGl0bGU+IDwvcmRmOkRlc2NyaXB0aW9uPiA8L3JkZjpSREY+IDwveDp4bXBtZXRhPiAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAg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"


class DiscImager:
    copy_thread = None

    def __init__(self, device):
        self.device = device
        self.notification = Notification("")
        self.notification.image = NOTIFICATION_IMAGE

    @staticmethod
    def needsImaging():
        if not os.path.exists(BOOTLOADER_IMAGE):
            logger.info(f"Bootloader image '{BOOTLOADER_IMAGE}' does not exist.")
            return False
        if not os.path.exists(BOOTLOADER_SCRIPT):
            logger.info(f"Bootloader script '{BOOTLOADER_SCRIPT}' does not exist.")
            return False
        if not os.path.exists(ROOTFS_ARCHIVE):
            logger.info(f"Root filesystem archive '{ROOTFS_ARCHIVE}' does not exist.")
            return False
        return True

    def create_partitions(self):
        self.updateNotication("Creating Partitions")
        device = parted.getDevice(self.device)
        logger.info(f"created {device}")

        sector_size = device.sectorSize
        logger.info(f"sector size is {sector_size}")

        disk = parted.freshDisk(device, "gpt")
        logger.info(f"created {disk}")
        # ../../../emmc.img1       64    16447    16384    8M Linux filesystem
        # ../../../emmc.img2    16448    32831    16384    8M Linux filesystem
        # ../../../emmc.img3    34816 10520575 10485760    5G EFI System
        # ../../../emmc.img4 10520576 21006335 10485760    5G EFI System
        # ../../../emmc.img5 21006336 30535643  9529308  4,5G Linux filesystem
        logger.info("Creating partitions...")
        for i, partition in enumerate(PARTITIONS):
            logger.info("\n\n----------------------\n\n")
            start = partition["start"] * 1024 // sector_size
            end = (partition["end"] * 1024 // sector_size) - 1

            name = partition["name"]
            fs = partition["fs"]
            size = (end - start) * sector_size // 1024
            logger.info(
                f"Partition: {partition['name']}, Start: {partition['start']} KiB, Size: {size} KiB"
            )
            geometry = parted.Geometry(
                device=device,
                start=start,
                end=end if end > start else start + 512 * 1024 // sector_size,
            )
            logger.info(f"created {geometry}")
            if i == len(PARTITIONS) - 1:
                free_space_regions = disk.getFreeSpaceRegions()
                geometry = free_space_regions[-1]

            filesystem = parted.FileSystem(type=fs, geometry=geometry)
            logger.info(f"created {filesystem}")

            # Turn off alignment constraints
            constraint = parted.Constraint(exactGeom=geometry, aligned=False)

            part = parted.Partition(
                disk=disk,
                type=parted.PARTITION_NORMAL,
                fs=filesystem,
                geometry=geometry,
            )
            part.set_name(name)
            logger.info(
                f"created {part}",
            )

            disk.addPartition(partition=part, constraint=constraint)
            if partition.get("bootable", False):
                # Mark uboot partition as bootable
                logger.info(f"Marking partition {name} as bootable")
                part.setFlag(parted.PARTITION_BOOT)
            logger.info(
                f"added partition {part}",
            )

        logger.info("Committing changes to disk...")
        disk.commit()
        logger.info("Partitions created successfully.")
        subprocess.run(f"partprobe {self.device}".split(), check=True)
        subprocess.run("sync", check=True)

    def format_partitions(self):
        self.updateNotication("Creating Filesystems")
        logger.info("Formatting partitions...")
        for i, partition in enumerate(PARTITIONS, start=1):
            part_device = f"{self.device}p{i}"
            fs_type = partition["fs"]
            if fs_type == "ext4":
                cmd = f"mkfs.ext4 -F {part_device}"
            elif fs_type == "fat32":
                cmd = f"mkfs.fat -F 32 {part_device}"
            else:
                logger.error(
                    f"Unknown filesystem type: {fs_type} for partition {partition['name']}"
                )
                continue
            logger.info(f"Formatting {part_device} as {fs_type}...")

            result = subprocess.run(cmd.split(), capture_output=True, text=True)
            logger.info(result.stdout)
            if result.returncode != 0:
                logger.error(f"Failed to format {part_device} as {fs_type}")
                logger.error(result.stderr)
            else:
                logger.info(f"Formatted {part_device} as {fs_type}")

        logger.info("syncing disks")
        result = subprocess.run("sync", check=True)

    def mount_partition(self, partition_number):

        part_device = f"{self.device}p{partition_number}"
        mountpoint = f"mount/part{partition_number}"
        os.makedirs(mountpoint, exist_ok=True)

        if os.path.ismount(mountpoint):
            logger.info(
                f"Partition {partition_number} is already mounted at {mountpoint}."
            )
            return mountpoint

        cmd = f"mount {part_device} {mountpoint}"

        result = subprocess.run(cmd.split(), capture_output=True, text=True)
        logger.info(result.stdout)
        if result.returncode != 0:
            logger.error(
                f"Failed to mount partition {partition_number}: {result.stderr}"
            )
            raise RuntimeError(
                f"Failed to mount partition {partition_number} with command '{cmd}'"
            )
        else:
            logger.info(f"Partition {partition_number} mounted successfully.")
            return mountpoint

    def unmount_partition(self, partition_number):
        part_device = f"{self.device}p{partition_number}"
        cmd = f"umount {part_device}"
        result = subprocess.run(cmd.split(), capture_output=True, text=True)
        logger.info(result.stdout)
        if result.returncode != 0:
            logger.info(
                f"Failed to unmount partition {partition_number}: {result.stderr}"
            )
            raise RuntimeError(
                f"Failed to unmount partition {partition_number} with command '{cmd}'"
            )
        else:
            logger.info(f"Partition {partition_number} unmounted successfully.")

    def write_bootloader(self):
        self.updateNotication("Writing bootloader...")
        logger.info("Writing bootloader...")
        cmd = f"cp {BOOTLOADER_IMAGE} {self.device}p1"
        result = subprocess.run(cmd.split(), capture_output=True, text=True)
        logger.info(result.stdout)
        if result.returncode != 0:
            logger.error(f"Failed to write bootloader: {result.stderr}")
            raise RuntimeError(f"Failed to write bootloader with command '{cmd}'")
        else:
            logger.info("Bootloader written successfully.")

    def write_bootloader_script(self):
        self.updateNotication("Writing bootloader scripts...")
        logger.info("Writing bootloader script...")
        mountpoint = self.mount_partition(2)
        if not mountpoint:
            logger.info("Failed to mount partition 2. Cannot write bootloader script.")
            return

        cmd = f"cp {BOOTLOADER_SCRIPT} {mountpoint}/u-boot.scr"
        result = subprocess.run(cmd.split(), capture_output=True, text=True)
        logger.info(result.stdout)
        if result.returncode != 0:
            logger.error(f"Failed to write bootloader script: {result.stderr}")
            raise RuntimeError("Failed to write bootloader script with command '{cmd}'")
        else:
            logger.info("Bootloader script written successfully.")

        cmd = f"cp {BOOTLOADER_SCRIPT} {mountpoint}/boot.scr"
        result = subprocess.run(cmd.split(), capture_output=True, text=True)
        logger.info(result.stdout)
        if result.returncode != 0:
            logger.error(f"Failed to write bootloader script: {result.stderr}")
            raise RuntimeError("Failed to write bootloader script with command '{cmd}'")
        else:
            logger.info("Bootloader script written successfully.")

        self.unmount_partition(2)

    def write_rootfs(self, partition_number=3):
        self.updateNotication("Writing root filesystem...")
        logger.info("Writing root filesystem...")
        mountpoint = self.mount_partition(partition_number)
        if not mountpoint:
            logger.error(
                f"Failed to mount partition {partition_number}. Cannot write root filesystem."
            )
            return
        in_cmd = [
            "pv",
            "-f",
            "--format",
            # for old PV from 2015
            '{"elapsed":"%t","bytes":"%b","rate":"%r","eta":"%e"}',
            # for more modern PV versions
            # '\'{"elapsed":%t,"bytes":%b,"rate":%r,"percentage":%{progress-amount-only}}\'',
            ROOTFS_ARCHIVE,
        ]

        tar_cmd = f"tar -x -C {mountpoint}"
        pv = subprocess.Popen(
            in_cmd,
            shell=False,
            bufsize=1,
            stderr=subprocess.PIPE,
            stdout=subprocess.PIPE,
            universal_newlines=True,
            env={"LANG": "C.UTF-8", "LC_ALL": "C.UTF-8"},
        )
        tar = subprocess.Popen(
            tar_cmd.split(),
            shell=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            stdin=pv.stdout,
        )
        pv.stdout.close()  # Allow pv to receive a SIGPIPE if mysql exits.
        for line in pv.stderr:
            json_data = line.rstrip("\n")
            progression = json.loads(json_data)
            logger.info(
                f"Elapsed: {progression['elapsed']}s - "
                f"Eta: {progression['eta']} - "
                f"Bytes: {progression['bytes']} - "
                f"Rate: {progression['rate']}"
            )
            self.updateNotication(
                f"Elapsed: {progression['elapsed']}s \n"
                f"Eta: {progression['eta']} \n"
                f"Bytes: {progression['bytes']} \n"
                f"Rate: {progression['rate']}"
            )

        pv_returncode = pv.wait()
        tar_returncode = tar.wait()
        if pv_returncode != 0:
            logger.error("Failed to read root filesystem archive")
            for line in pv.stderr:
                logger.info(line.rstrip("\n").encode().decode())
            logger.error(f"Return code: {pv_returncode}")
            raise RuntimeError(
                f"PV Failed to read root filesystem archive with return code {pv_returncode}"
            )
        if tar_returncode != 0:
            logger.error("Failed to write root filesystem")
            for line in tar.stderr:
                logger.error(line.rstrip("\n".encode()).decode())
            logger.error(f"Return code: {tar_returncode}")
            raise RuntimeError(
                f"TAR Failed to write root filesystem with return code {tar_returncode}"
            )
        self.updateNotication(
            f"Root filesystem written successfully to partition {partition_number}",
            responses=[NotificationResponse.OK],
        )
        self.unmount_partition(partition_number)

    def updateNotication(self, message, responses=None):
        self.notification.message = message
        if responses is not None:
            self.notification.responses = responses
        else:
            self.notification.responses = [
                NotificationResponse.OK,
            ]
        NotificationManager.add_notification(self.notification)

    @staticmethod
    def flash():
        logger.info("Starting to image emmc")
        waitTime = 10
        imager = DiscImager(device="/dev/mmcblk2")
        time.sleep(waitTime)

        imager.updateNotication(
            f"Starting to image emmc in {waitTime} seconds",
            responses=[
                NotificationResponse.OK,
                NotificationResponse.SKIP,
            ],
        )
        time.sleep(waitTime)
        if (
            imager.notification.acknowledged
            and imager.notification.response == NotificationResponse.SKIP
        ):
            return

        start_time = time.time()
        try:
            imager.create_partitions()
            imager.format_partitions()
            imager.write_bootloader()
            imager.write_bootloader_script()
            imager.write_rootfs(partition_number=3)
        except Exception as e:
            logger.error(f"An error occurred during imaging: {e}")
            traceback.print_exc()
            imager.updateNotication(
                f"An error occurred during imaging: {e}\nCheck the logs for details.",
                responses=[NotificationResponse.OK],
            )
            return
        end_time = time.time()
        elapsed = end_time - start_time
        logger.info(f"Total imaging time: {elapsed:.2f} seconds")
        imager.updateNotication(
            f"Imaging completed successfully in {elapsed:.2f} seconds",
            responses=[NotificationResponse.OK],
        )

    @staticmethod
    def flash_if_required():
        if not DiscImager.needsImaging():
            return
        logger.info("Imaging is required. Provisioning the device...")
        DiscImager.copy_thread = NamedThread("DiscImager", target=DiscImager.flash)
        DiscImager.copy_thread.start()
