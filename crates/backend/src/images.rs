//! Notification images (`images/notificationImages/base64.py`): data URIs the
//! Dial shows next to a notification.

pub const FLASHING_NOTIFICATION_IMAGE: &str =
    include_str!("../assets/notification_images/flashing_notification_image.txt");
pub const USB_DEVICE_NOTIFICATION_IMAGE: &str =
    include_str!("../assets/notification_images/usb_device_notification_image.txt");
pub const CHECKMARK_IMAGE: &str = include_str!("../assets/notification_images/checkmark_image.txt");
pub const ERROR_IMAGE: &str = include_str!("../assets/notification_images/error_image.txt");
pub const WARNING_TRIANGLE_IMAGE: &str =
    include_str!("../assets/notification_images/warning_triangle_image.txt");
