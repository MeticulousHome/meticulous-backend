#!/usr/bin/env python3
# -*- coding: utf-8 -*-

import os.path
import parted
import subprocess
import json
import traceback
import time
from named_thread import NamedThread
import datetime
from timezone_manager import TimezoneManager

from notifications import NotificationManager, Notification, NotificationResponse
from images.notificationImages.base64 import (
    FLASHING_NOTIFICATION_IMAGE as NOTIFICATION_IMAGE,
    CHECKMARK_IMAGE,
    ERROR_IMAGE,
)

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

        # get fs creation date
        date_cmd = f"stat {ROOTFS_ARCHIVE} | grep Birth"
        try:
            ROOTFS_BIRTH = subprocess.run(
                date_cmd, shell=True, text=True, capture_output=True, check=True
            )
            birth_str = ROOTFS_BIRTH.stdout.strip()
            if birth_str.find("Birth") == -1:
                raise Exception("unexpected result from 'stat'")
            birth_data = birth_str.split()[1:3]
            logger.warning(f"birth data: {birth_data}")
            if "." in birth_data[1]:
                b_time = birth_data[1].split(".")[0]
            birth_date = datetime.datetime.strptime(
                f"{birth_data[0]} {b_time}", "%Y-%m-%d %H:%M:%S"
            )
            new_date = birth_date + datetime.timedelta(days=1)
            TimezoneManager.set_system_datetime(new_date)
            logger.warning(
                f'system date set to {new_date.strftime("%Y-%m-%d %H:%M:%S")}'
            )

        except Exception as e:
            logger.warning(
                f"could not set system dat to ${ROOTFS_ARCHIVE} birth date: {e}"
            )

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
            stderr=subprocess.DEVNULL,
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

    def updateNotication(self, message, responses=None, image=None):
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
                image=ERROR_IMAGE,
            )
            return
        end_time = time.time()
        elapsed = end_time - start_time
        logger.info(f"Total imaging time: {elapsed:.2f} seconds")
        imager.updateNotication(
            f"Imaging completed successfully in {elapsed:.2f} seconds",
            responses=[NotificationResponse.OK],
            image=CHECKMARK_IMAGE,
        )

    @staticmethod
    def flash_if_required():
        if not DiscImager.needsImaging():
            return
        logger.info("Imaging is required. Provisioning the device...")
        DiscImager.copy_thread = NamedThread("DiscImager", target=DiscImager.flash)
        DiscImager.copy_thread.start()
