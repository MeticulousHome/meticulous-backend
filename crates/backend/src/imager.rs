//! `imager.py`: manufacturing provisioning. When the provisioning files are
//! on the user partition, the eMMC (`/dev/mmcblk2`) is partitioned (GPT),
//! formatted, given the bootloader and its script, and the root filesystem
//! archive is unpacked onto `root_a`, with a notification at each step.
//!
//! The Python partitions with libparted (pyparted); here the GPT is written
//! directly, with libparted's layout: exact sectors from the table below, no
//! alignment, the last partition filling the disk, Microsoft basic data for
//! FAT, Linux filesystem for ext4 and the EFI system type for partitions
//! flagged bootable.

use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::host::Host;
use crate::images::FLASHING_NOTIFICATION_IMAGE;
use crate::notifications::{Notification, NotificationManager, responses};

const DEVICE: &str = "/dev/mmcblk2";
const WAIT_TIME: Duration = Duration::from_secs(10);

/// `PARTITIONS`: start and end in KiB.
struct PartitionSpec {
    name: &'static str,
    start_kib: u64,
    end_kib: u64,
    fs: &'static str,
    bootable: bool,
}

const PARTITIONS: [PartitionSpec; 5] = [
    PartitionSpec {
        name: "uboot",
        start_kib: 0x000020,
        end_kib: 0x002020,
        fs: "fat32",
        bootable: false,
    },
    PartitionSpec {
        name: "uboot_env",
        start_kib: 0x002020,
        end_kib: 0x004020,
        fs: "fat32",
        bootable: false,
    },
    PartitionSpec {
        name: "root_a",
        start_kib: 0x004400,
        end_kib: 0x504400,
        fs: "ext4",
        bootable: true,
    },
    PartitionSpec {
        name: "root_b",
        start_kib: 0x504400,
        end_kib: 0xA04400,
        fs: "ext4",
        bootable: true,
    },
    // The user partition fills the rest of the disk.
    PartitionSpec {
        name: "user",
        start_kib: 0xA04400,
        end_kib: 0xA04400 + 0x2000,
        fs: "ext4",
        bootable: false,
    },
];

pub struct DiscImager {
    notifications: Arc<NotificationManager>,
    host: Arc<Host>,
    provisioning: PathBuf,
    fake_hardware: bool,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

/// One flashing run: the device and the notification it keeps updating.
struct Imaging {
    device: String,
    notification: Notification,
    notifications: Arc<NotificationManager>,
    host: Arc<Host>,
    bootloader_image: PathBuf,
    bootloader_script: PathBuf,
    rootfs_archive: PathBuf,
}

/// Python's `RuntimeError(message)` and friends, as `str(e)`.
type ImagingError = String;

impl DiscImager {
    pub fn new(
        notifications: Arc<NotificationManager>,
        host: Arc<Host>,
        provisioning: PathBuf,
        fake_hardware: bool,
    ) -> DiscImager {
        DiscImager {
            notifications,
            host,
            provisioning,
            fake_hardware,
            thread: Mutex::new(None),
        }
    }

    fn bootloader_image(&self) -> PathBuf {
        self.provisioning.join("imx-boot-sd.bin")
    }

    fn bootloader_script(&self) -> PathBuf {
        self.provisioning.join("u-boot.scr")
    }

    fn rootfs_archive(&self) -> PathBuf {
        self.provisioning.join("meticulous-rootfs.tar")
    }

    /// `DiscImager.needsImaging`: all three provisioning files are present.
    pub fn needs_imaging(&self) -> bool {
        if self.fake_hardware {
            // The boot harness patches this to False: never touch a disk.
            return false;
        }
        for (path, what) in [
            (self.bootloader_image(), "Bootloader image"),
            (self.bootloader_script(), "Bootloader script"),
            (self.rootfs_archive(), "Root filesystem archive"),
        ] {
            if !path.exists() {
                tracing::info!("{what} '{}' does not exist.", path.display());
                return false;
            }
        }
        true
    }

    /// `DiscImager.flash_if_required`: image the eMMC on its own thread.
    pub fn flash_if_required(&self) {
        if !self.needs_imaging() {
            return;
        }
        tracing::info!("Imaging is required. Provisioning the device...");
        let imaging = Imaging {
            device: DEVICE.to_string(),
            notification: Notification::new("").image(FLASHING_NOTIFICATION_IMAGE),
            notifications: self.notifications.clone(),
            host: self.host.clone(),
            bootloader_image: self.bootloader_image(),
            bootloader_script: self.bootloader_script(),
            rootfs_archive: self.rootfs_archive(),
        };
        match std::thread::Builder::new()
            .name("DiscImager".into())
            .spawn(move || imaging.flash())
        {
            Ok(handle) => *self.thread.lock() = Some(handle),
            Err(error) => tracing::error!("Could not start imaging: {error}"),
        }
    }
}

impl Imaging {
    /// `updateNotication`. PARITY: the Python assigns `responses` and `image`
    /// to attributes the notification never sends, so the notification always
    /// offers "Ok" and keeps the flashing image.
    fn update_notification(&mut self, message: &str) {
        self.notification.message = message.to_string();
        self.notification.responses = vec![responses::OK.to_string()];
        self.notifications.add(self.notification.clone());
    }

    /// `DiscImager.flash`.
    fn flash(mut self) {
        tracing::info!("Starting to image emmc");
        std::thread::sleep(WAIT_TIME);
        self.update_notification(&format!(
            "Starting to image emmc in {} seconds",
            WAIT_TIME.as_secs()
        ));
        std::thread::sleep(WAIT_TIME);
        let skipped = self
            .notifications
            .all()
            .into_iter()
            .find(|n| n.id == self.notification.id)
            .is_some_and(|n| n.acknowledged && n.response.as_deref() == Some(responses::SKIP));
        if skipped {
            return;
        }

        let start = Instant::now();
        let result = self
            .create_partitions()
            .and_then(|()| self.format_partitions())
            .and_then(|()| self.write_bootloader())
            .and_then(|()| self.write_bootloader_script())
            .and_then(|()| self.write_rootfs(3));
        if let Err(error) = result {
            tracing::error!("An error occurred during imaging: {error}");
            self.update_notification(&format!(
                "An error occurred during imaging: {error}\nCheck the logs for details."
            ));
            return;
        }
        let elapsed = start.elapsed().as_secs_f64();
        tracing::info!("Total imaging time: {elapsed:.2} seconds");
        self.update_notification(&format!(
            "Imaging completed successfully in {elapsed:.2} seconds"
        ));
    }

    fn part_device(&self, number: usize) -> String {
        format!("{}p{number}", self.device)
    }

    /// Run a command with its output captured; its stdout goes to the log.
    fn run_logged(&self, argv: &[&str]) -> Result<crate::host::CommandOutput, ImagingError> {
        let output = self.host.run(argv).map_err(|e| {
            format!(
                "[Errno {}] {e}: '{}'",
                e.raw_os_error().unwrap_or(0),
                argv[0]
            )
        })?;
        tracing::info!("{}", output.stdout_text());
        Ok(output)
    }

    fn check_call(&self, argv: &[&str]) -> Result<(), ImagingError> {
        let output = self.host.run(argv).map_err(|e| {
            format!(
                "[Errno {}] {e}: '{}'",
                e.raw_os_error().unwrap_or(0),
                argv[0]
            )
        })?;
        if output.success() {
            Ok(())
        } else {
            Err(crate::host::describe_failure(argv, &output))
        }
    }

    /// `create_partitions`.
    fn create_partitions(&mut self) -> Result<(), ImagingError> {
        self.update_notification("Creating Partitions");
        let sector_size = logical_sector_size(&self.device);
        tracing::info!("sector size is {sector_size}");
        let mut disk = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.device)
            .map_err(|e| format!("Error opening {}: {e}", self.device))?;
        let total_sectors = disk.seek(SeekFrom::End(0)).map_err(|e| e.to_string())? / sector_size;
        tracing::info!("Creating partitions...");
        let layout = partition_layout(sector_size, total_sectors)?;
        for (spec, part) in PARTITIONS.iter().zip(&layout) {
            tracing::info!(
                "Partition: {}, Start: {} KiB, Size: {} KiB",
                spec.name,
                spec.start_kib,
                (part.last_lba - part.first_lba) * sector_size / 1024
            );
            if spec.bootable {
                tracing::info!("Marking partition {} as bootable", spec.name);
            }
        }
        tracing::info!("Committing changes to disk...");
        write_gpt(&mut disk, sector_size, total_sectors, &layout).map_err(|e| e.to_string())?;
        disk.sync_all().map_err(|e| e.to_string())?;
        drop(disk);
        tracing::info!("Partitions created successfully.");
        self.check_call(&["partprobe", &self.device.clone()])?;
        self.check_call(&["sync"])
    }

    /// `format_partitions`: a failed mkfs is logged and imaging goes on.
    fn format_partitions(&mut self) -> Result<(), ImagingError> {
        self.update_notification("Creating Filesystems");
        tracing::info!("Formatting partitions...");
        for (i, spec) in PARTITIONS.iter().enumerate() {
            let part = self.part_device(i + 1);
            let argv: Vec<&str> = match spec.fs {
                "ext4" => vec!["mkfs.ext4", "-F", &part],
                "fat32" => vec!["mkfs.fat", "-F", "32", &part],
                other => {
                    tracing::error!(
                        "Unknown filesystem type: {other} for partition {}",
                        spec.name
                    );
                    continue;
                }
            };
            tracing::info!("Formatting {part} as {}...", spec.fs);
            let output = self.run_logged(&argv)?;
            if output.success() {
                tracing::info!("Formatted {part} as {}", spec.fs);
            } else {
                tracing::error!("Failed to format {part} as {}", spec.fs);
                tracing::error!("{}", String::from_utf8_lossy(&output.stderr));
            }
        }
        tracing::info!("syncing disks");
        self.check_call(&["sync"])
    }

    /// `mount_partition`. PARITY: the mount point is relative to the
    /// working directory, as in the Python.
    fn mount_partition(&self, number: usize) -> Result<String, ImagingError> {
        let part = self.part_device(number);
        let mountpoint = format!("mount/part{number}");
        std::fs::create_dir_all(&mountpoint).map_err(|e| e.to_string())?;
        if is_mount(Path::new(&mountpoint)) {
            tracing::info!("Partition {number} is already mounted at {mountpoint}.");
            return Ok(mountpoint);
        }
        let output = self.run_logged(&["mount", &part, &mountpoint])?;
        if !output.success() {
            tracing::error!(
                "Failed to mount partition {number}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            return Err(format!(
                "Failed to mount partition {number} with command 'mount {part} {mountpoint}'"
            ));
        }
        tracing::info!("Partition {number} mounted successfully.");
        Ok(mountpoint)
    }

    fn unmount_partition(&self, number: usize) -> Result<(), ImagingError> {
        let part = self.part_device(number);
        let output = self.run_logged(&["umount", &part])?;
        if !output.success() {
            tracing::info!(
                "Failed to unmount partition {number}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            return Err(format!(
                "Failed to unmount partition {number} with command 'umount {part}'"
            ));
        }
        tracing::info!("Partition {number} unmounted successfully.");
        Ok(())
    }

    /// `write_bootloader`: the boot image into partition 1 (32 KiB in, where
    /// the i.MX boot ROM looks).
    fn write_bootloader(&mut self) -> Result<(), ImagingError> {
        self.update_notification("Writing bootloader...");
        tracing::info!("Writing bootloader...");
        let image = self.bootloader_image.to_string_lossy().into_owned();
        let target = self.part_device(1);
        let output = self.run_logged(&["cp", &image, &target])?;
        if !output.success() {
            tracing::error!(
                "Failed to write bootloader: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            return Err(format!(
                "Failed to write bootloader with command 'cp {image} {target}'"
            ));
        }
        tracing::info!("Bootloader written successfully.");
        Ok(())
    }

    /// `write_bootloader_script`: `u-boot.scr` and `boot.scr` on partition 2.
    fn write_bootloader_script(&mut self) -> Result<(), ImagingError> {
        self.update_notification("Writing bootloader scripts...");
        tracing::info!("Writing bootloader script...");
        let mountpoint = self.mount_partition(2)?;
        let script = self.bootloader_script.to_string_lossy().into_owned();
        for name in ["u-boot.scr", "boot.scr"] {
            let target = format!("{mountpoint}/{name}");
            let output = self.run_logged(&["cp", &script, &target])?;
            if !output.success() {
                tracing::error!(
                    "Failed to write bootloader script: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                // PARITY: the Python forgot the f-string prefix.
                return Err("Failed to write bootloader script with command '{cmd}'".into());
            }
            tracing::info!("Bootloader script written successfully.");
        }
        self.unmount_partition(2)
    }

    /// The system clock to a day after the archive was made, so the
    /// unpacked files are not dated in the future.
    fn set_date_from_archive(&self) {
        let command = format!("stat {} | grep Birth", self.rootfs_archive.display());
        let result = (|| -> Result<(), &'static str> {
            let output = self.host.run_shell(&command).map_err(|_| "OSError")?;
            if !output.success() {
                return Err("CalledProcessError");
            }
            let birth = output.stdout_text();
            let birth = birth.trim();
            if !birth.contains("Birth") {
                return Err("Exception");
            }
            let fields: Vec<&str> = birth.split_whitespace().skip(1).take(2).collect();
            tracing::warn!("birth data: {fields:?}");
            let time = fields.get(1).ok_or("IndexError")?;
            let Some((time, _)) = time.split_once('.') else {
                // PARITY: b_time is only set when the time has a fraction.
                return Err("UnboundLocalError");
            };
            let birth = chrono::NaiveDateTime::parse_from_str(
                &format!("{} {time}", fields[0]),
                "%Y-%m-%d %H:%M:%S",
            )
            .map_err(|_| "ValueError")?;
            let new_date = birth + chrono::Duration::days(1);
            crate::clock::set_system_datetime(&self.host, &new_date).map_err(|e| match e {
                crate::clock::SetTimeError::NotStarted(_) => "FileNotFoundError",
                crate::clock::SetTimeError::Failed(_) => "CalledProcessError",
            })?;
            tracing::warn!(
                "system date set to {}",
                crate::clock::format_datetime(&new_date)
            );
            Ok(())
        })();
        if let Err(kind) = result {
            tracing::warn!(
                "could not set system dat to ${} birth date: {kind}",
                self.rootfs_archive.display()
            );
        }
    }

    /// `write_rootfs`: `pv <archive> | tar -x -C <mountpoint>`, pv's
    /// progress shown in the notification.
    fn write_rootfs(&mut self, number: usize) -> Result<(), ImagingError> {
        self.update_notification("Writing root filesystem...");
        tracing::info!("Writing root filesystem...");
        let mountpoint = self.mount_partition(number)?;
        self.set_date_from_archive();

        let mut pv = Command::new("pv")
            .args([
                "-f",
                "--format",
                r#"{"elapsed":"%t","bytes":"%b","rate":"%r","eta":"%e"}"#,
            ])
            .arg(&self.rootfs_archive)
            .env_clear()
            .env("LANG", "C.UTF-8")
            .env("LC_ALL", "C.UTF-8")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("[Errno {}] {e}: 'pv'", e.raw_os_error().unwrap_or(0)))?;
        let pv_out = pv.stdout.take().ok_or("pv has no stdout")?;
        let mut tar = Command::new("tar")
            .args(["-x", "-C", &mountpoint])
            .stdin(Stdio::from(pv_out))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("[Errno {}] {e}: 'tar'", e.raw_os_error().unwrap_or(0)))?;

        if let Some(mut progress) = pv.stderr.take() {
            let mut pending = Vec::new();
            let mut buffer = [0u8; 4096];
            let mut after_cr = false;
            loop {
                let read = progress.read(&mut buffer).map_err(|e| e.to_string())?;
                if read == 0 {
                    break;
                }
                pending.extend_from_slice(&buffer[..read]);
                while let Some(end) = pending.iter().position(|b| *b == b'\r' || *b == b'\n') {
                    let line: Vec<u8> = pending.drain(..=end).collect();
                    // "\r\n" is one line break in universal newlines mode.
                    let skip = after_cr && line == b"\n";
                    after_cr = line.last() == Some(&b'\r');
                    if skip {
                        continue;
                    }
                    let text = String::from_utf8_lossy(&line[..line.len() - 1]).into_owned();
                    let message = pv_progress_message(&text)?;
                    self.update_notification(&message);
                }
            }
        }

        let pv_status = pv.wait().map_err(|e| e.to_string())?;
        let tar_status = tar.wait().map_err(|e| e.to_string())?;
        if !pv_status.success() {
            tracing::error!("Failed to read root filesystem archive");
            let code = pv_status.code().unwrap_or(-1);
            tracing::error!("Return code: {code}");
            return Err(format!(
                "PV Failed to read root filesystem archive with return code {code}"
            ));
        }
        if !tar_status.success() {
            tracing::error!("Failed to write root filesystem");
            // PARITY: the Python iterates tar's stderr, which is DEVNULL
            // (None), and fails with that TypeError before its own message.
            return Err("'NoneType' object is not iterable".into());
        }
        self.update_notification(&format!(
            "Root filesystem written successfully to partition {number}"
        ));
        self.unmount_partition(number)
    }
}

/// One pv progress line as the log and the notification show it.
fn pv_progress_message(line: &str) -> Result<String, ImagingError> {
    let progress = pyfmt::loads_lenient(line).map_err(|e| e.to_string())?;
    let field = |key: &str| -> Result<String, ImagingError> {
        progress
            .get(key)
            .map(crate::ota::python_str)
            .ok_or_else(|| format!("'{key}'"))
    };
    let (elapsed, eta, bytes, rate) = (
        field("elapsed")?,
        field("eta")?,
        field("bytes")?,
        field("rate")?,
    );
    tracing::info!("Elapsed: {elapsed}s - Eta: {eta} - Bytes: {bytes} - Rate: {rate}");
    Ok(format!(
        "Elapsed: {elapsed}s \nEta: {eta} \nBytes: {bytes} \nRate: {rate}"
    ))
}

/// `os.path.ismount`.
fn is_mount(path: &Path) -> bool {
    let (Ok(here), Ok(parent)) = (std::fs::metadata(path), std::fs::metadata(path.join("..")))
    else {
        return false;
    };
    here.dev() != parent.dev() || here.ino() == parent.ino()
}

/// The device's logical sector size, as libparted reads it.
fn logical_sector_size(device: &str) -> u64 {
    let name = Path::new(device)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    std::fs::read_to_string(format!("/sys/class/block/{name}/queue/logical_block_size"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .filter(|size| *size >= 512)
        .unwrap_or(512)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GptPartition {
    name: &'static str,
    type_guid: uuid::Uuid,
    first_lba: u64,
    /// Inclusive.
    last_lba: u64,
}

const MICROSOFT_BASIC_DATA: uuid::Uuid = uuid::uuid!("ebd0a0a2-b9e5-4433-87c0-68b6b72699c7");
const LINUX_FILESYSTEM: uuid::Uuid = uuid::uuid!("0fc63daf-8483-4772-8e79-3d69d8477de4");
const EFI_SYSTEM: uuid::Uuid = uuid::uuid!("c12a7328-f81f-11d2-ba4b-00a0c93ec93b");
const GPT_ENTRIES: u64 = 128;
const GPT_ENTRY_SIZE: u64 = 128;

fn entry_sectors(sector_size: u64) -> u64 {
    (GPT_ENTRIES * GPT_ENTRY_SIZE).div_ceil(sector_size)
}

/// The partitions `create_partitions` asks libparted for.
fn partition_layout(
    sector_size: u64,
    total_sectors: u64,
) -> Result<Vec<GptPartition>, ImagingError> {
    let last_usable = total_sectors
        .checked_sub(2 + entry_sectors(sector_size))
        .ok_or("disk too small")?;
    let mut layout = Vec::new();
    for (i, spec) in PARTITIONS.iter().enumerate() {
        let start = spec.start_kib * 1024 / sector_size;
        let end = (spec.end_kib * 1024 / sector_size).saturating_sub(1);
        let mut part = GptPartition {
            name: spec.name,
            type_guid: if spec.bootable {
                EFI_SYSTEM
            } else if spec.fs.starts_with("fat") {
                MICROSOFT_BASIC_DATA
            } else {
                LINUX_FILESYSTEM
            },
            first_lba: start,
            last_lba: if end > start {
                end
            } else {
                start + 512 * 1024 / sector_size
            },
        };
        if i == PARTITIONS.len() - 1 {
            // The last free region: after the previous partition, up to the
            // last usable sector.
            let previous_end = layout
                .last()
                .map(|p: &GptPartition| p.last_lba)
                .unwrap_or(0);
            part.first_lba = previous_end + 1;
            part.last_lba = last_usable;
        }
        if part.last_lba > last_usable || part.first_lba > part.last_lba {
            return Err(format!(
                "Can't have a partition outside the disk! ({})",
                spec.name
            ));
        }
        layout.push(part);
    }
    Ok(layout)
}

/// A fresh GPT: protective MBR (keeping the old boot code), primary header
/// and entries, backup entries and header.
fn write_gpt<D: Read + Write + Seek>(
    disk: &mut D,
    sector_size: u64,
    total_sectors: u64,
    partitions: &[GptPartition],
) -> std::io::Result<()> {
    let ss = sector_size as usize;
    let entries_len = entry_sectors(sector_size);
    let last_lba = total_sectors - 1;
    let first_usable = 2 + entries_len;
    let last_usable = last_lba - 1 - entries_len;

    let mut entries = vec![0u8; (entries_len * sector_size) as usize];
    for (i, part) in partitions.iter().enumerate() {
        let entry = &mut entries[i * GPT_ENTRY_SIZE as usize..(i + 1) * GPT_ENTRY_SIZE as usize];
        entry[0..16].copy_from_slice(&part.type_guid.to_bytes_le());
        entry[16..32].copy_from_slice(&uuid::Uuid::new_v4().to_bytes_le());
        entry[32..40].copy_from_slice(&part.first_lba.to_le_bytes());
        entry[40..48].copy_from_slice(&part.last_lba.to_le_bytes());
        for (j, unit) in part.name.encode_utf16().take(36).enumerate() {
            entry[56 + 2 * j..58 + 2 * j].copy_from_slice(&unit.to_le_bytes());
        }
    }
    let entries_crc = crc32fast::hash(&entries[..(GPT_ENTRIES * GPT_ENTRY_SIZE) as usize]);
    let disk_guid = uuid::Uuid::new_v4();

    let header = |current: u64, backup: u64, entries_lba: u64| -> Vec<u8> {
        let mut h = vec![0u8; ss];
        h[0..8].copy_from_slice(b"EFI PART");
        h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        h[12..16].copy_from_slice(&92u32.to_le_bytes());
        h[24..32].copy_from_slice(&current.to_le_bytes());
        h[32..40].copy_from_slice(&backup.to_le_bytes());
        h[40..48].copy_from_slice(&first_usable.to_le_bytes());
        h[48..56].copy_from_slice(&last_usable.to_le_bytes());
        h[56..72].copy_from_slice(&disk_guid.to_bytes_le());
        h[72..80].copy_from_slice(&entries_lba.to_le_bytes());
        h[80..84].copy_from_slice(&(GPT_ENTRIES as u32).to_le_bytes());
        h[84..88].copy_from_slice(&(GPT_ENTRY_SIZE as u32).to_le_bytes());
        h[88..92].copy_from_slice(&entries_crc.to_le_bytes());
        let crc = crc32fast::hash(&h[..92]);
        h[16..20].copy_from_slice(&crc.to_le_bytes());
        h
    };

    let mut mbr = vec![0u8; ss];
    disk.seek(SeekFrom::Start(0))?;
    let _ = disk.read(&mut mbr[..446])?;
    mbr[446..510].fill(0);
    let record = &mut mbr[446..462];
    record[1..4].copy_from_slice(&[0x00, 0x02, 0x00]);
    record[4] = 0xEE;
    record[5..8].copy_from_slice(&[0xFF, 0xFF, 0xFF]);
    record[8..12].copy_from_slice(&1u32.to_le_bytes());
    let size = u32::try_from(total_sectors - 1).unwrap_or(u32::MAX);
    record[12..16].copy_from_slice(&size.to_le_bytes());
    mbr[510] = 0x55;
    mbr[511] = 0xAA;

    let write_at = |disk: &mut D, lba: u64, bytes: &[u8]| -> std::io::Result<()> {
        disk.seek(SeekFrom::Start(lba * sector_size))?;
        disk.write_all(bytes)
    };
    write_at(disk, 2, &entries)?;
    write_at(disk, 1, &header(1, last_lba, 2))?;
    write_at(disk, last_lba - entries_len, &entries)?;
    write_at(disk, last_lba, &header(last_lba, 1, last_lba - entries_len))?;
    write_at(disk, 0, &mbr)?;
    disk.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The layout libparted produced for the Python on a 15.6 GB eMMC
    /// (the table in imager.py's comment).
    #[test]
    fn partition_layout_matches_the_python() {
        let layout = partition_layout(512, 30_535_680).expect("layout");
        let sectors: Vec<(u64, u64)> = layout.iter().map(|p| (p.first_lba, p.last_lba)).collect();
        assert_eq!(
            sectors,
            vec![
                (64, 16447),
                (16448, 32831),
                (34816, 10520575),
                (10520576, 21006335),
                (21006336, 30535646),
            ]
        );
        assert_eq!(layout[0].type_guid, MICROSOFT_BASIC_DATA);
        assert_eq!(layout[2].type_guid, EFI_SYSTEM);
        assert_eq!(layout[3].type_guid, EFI_SYSTEM);
        assert_eq!(layout[4].type_guid, LINUX_FILESYSTEM);
        assert!(partition_layout(512, 20_000_000).is_err());
    }

    /// The GPT reads back: signatures, CRCs, entries, the backup copy.
    #[test]
    fn gpt_is_well_formed() {
        let total = 30_535_680u64 / 1024; // a small disk is enough for the format
        let parts = vec![GptPartition {
            name: "uboot",
            type_guid: MICROSOFT_BASIC_DATA,
            first_lba: 64,
            last_lba: 16447.min(total - 40),
        }];
        let mut disk = std::io::Cursor::new(vec![0xAAu8; (total * 512) as usize]);
        write_gpt(&mut disk, 512, total, &parts).expect("gpt");
        let bytes = disk.into_inner();
        assert_eq!(&bytes[510..512], &[0x55, 0xAA]);
        assert_eq!(bytes[446 + 4], 0xEE);
        assert_eq!(&bytes[..4], &[0xAA; 4], "boot code kept");
        let header = &bytes[512..1024];
        assert_eq!(&header[..8], b"EFI PART");
        let mut zeroed = header[..92].to_vec();
        zeroed[16..20].fill(0);
        assert_eq!(crc32fast::hash(&zeroed).to_le_bytes(), header[16..20]);
        let entries = &bytes[1024..1024 + 128 * 128];
        assert_eq!(crc32fast::hash(entries).to_le_bytes(), header[88..92]);
        assert_eq!(&entries[0..16], &MICROSOFT_BASIC_DATA.to_bytes_le());
        assert_eq!(
            u64::from_le_bytes(entries[32..40].try_into().expect("8")),
            64
        );
        assert_eq!(
            &entries[56..66],
            &[b'u', 0, b'b', 0, b'o', 0, b'o', 0, b't', 0]
        );
        let backup = &bytes[((total - 1) * 512) as usize..(total * 512) as usize];
        assert_eq!(&backup[..8], b"EFI PART");
        assert_eq!(
            u64::from_le_bytes(backup[24..32].try_into().expect("8")),
            total - 1
        );
        assert_eq!(
            u64::from_le_bytes(backup[72..80].try_into().expect("8")),
            total - 33
        );
    }

    #[test]
    fn pv_progress_lines() {
        assert_eq!(
            pv_progress_message(
                r#"{"elapsed":"0:00:01","bytes":"10.0MiB","rate":"[10.0MiB/s]","eta":"0:01:00"}"#
            ),
            Ok("Elapsed: 0:00:01s \nEta: 0:01:00 \nBytes: 10.0MiB \nRate: [10.0MiB/s]".to_string())
        );
        assert!(pv_progress_message("garbage").is_err());
    }

    /// Without the provisioning files, or with the hardware faked, nothing
    /// is imaged.
    #[test]
    fn imaging_needs_all_provisioning_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = Arc::new(crate::config::ConfigStore::open(
            dir.path().join("config.yml"),
            crate::config::defaults(),
        ));
        let sio = Arc::new(crate::sio::Sio::default());
        let sounds = Arc::new(crate::sounds::SoundPlayer::new(
            config.clone(),
            dir.path().join("s"),
            dir.path().join("u"),
            true,
        ));
        let notifications = Arc::new(NotificationManager::new(sio, config, sounds));
        let provisioning = dir.path().join("provisioning");
        let imager = DiscImager::new(
            notifications.clone(),
            Arc::new(Host::default()),
            provisioning.clone(),
            false,
        );
        assert!(!imager.needs_imaging());
        std::fs::create_dir_all(&provisioning).expect("dir");
        for name in ["imx-boot-sd.bin", "u-boot.scr"] {
            std::fs::write(provisioning.join(name), "").expect("file");
        }
        assert!(!imager.needs_imaging());
        std::fs::write(provisioning.join("meticulous-rootfs.tar"), "").expect("file");
        assert!(imager.needs_imaging());
        let faked = DiscImager::new(notifications, Arc::new(Host::default()), provisioning, true);
        assert!(!faked.needs_imaging());
    }
}
