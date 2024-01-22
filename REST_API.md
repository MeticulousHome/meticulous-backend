# Meticulous Backend REST API Documentation

## Profile Management

This endpoint provides functionalities for listing, saving, loading, and retrieving user profiles.

### 1. List Profiles (`/profile/list`)

#### GET `/profile/list`

Retrieves a list of all available profiles.

**Response:**

- `200 OK`: Successful retrieval.
  - Returns an array of profile summaries, each containing profile details except for the `stages` attribute.

### 2. Save Profile (`/profile/save/{save_id}`)

#### POST `/profile/save/{save_id}`

Saves a new profile or updates an existing one based on the provided ID.

**Path Parameters:**

- `save_id`: ID used for saving the profile.

**Request Body:**

- JSON object representing the profile data to be saved.

**Response:**

- `200 OK`: Profile saved successfully.
  - Returns an object with `name` and `uuid` of the saved profile.
- `400 Bad Request`: Error occurred while saving the profile.

### 3. Load Profile (`/profile/load`)

#### GET `/profile/load/{profile_id}`

Loads a specific profile by its ID.

**Path Parameters:**

- `profile_id`: Unique identifier of the profile to load.

**Response:**

- `200 OK`: Profile loaded successfully.
  - Returns an object with `name` and `uuid` of the loaded profile.
- `404 Not Found`: No profile found with the provided UUID.

#### POST `/profile/load`

Loads a profile from the provided data temporarily.

**Request Body:**

- JSON object representing the profile data to be loaded.

**Response:**

- `200 OK`: Temporary profile loaded successfully.
  - Returns an object with `name` and `uuid` of the loaded profile.
- `400 Bad Request`: Error occurred while loading the profile.

### 4. Get Profile (`/profile/get/{profile_id}`)

#### GET `/profile/get/{profile_id}`

Retrieves the detailed data of a specific profile.

**Path Parameters:**

- `profile_id`: Unique identifier of the profile to retrieve.

**Response:**

- `200 OK`: Successful retrieval of the profile data.
- `404 Not Found`: No profile found with the provided UUID.

## Wifi

The endpoint provides functionality for WiFi configuration, listing available networks, and connecting to a WiFi network.

### 1. WiFi Configuration (`/wifi/config`)

#### GET `/wifi/config`

Retrieves the current WiFi configuration and status.

**Response:**

- `200 OK`: Successful retrieval.
  - `config`: Object containing WiFi configuration details.
    - `provisioning`: Boolean indicating if provisioning is enabled.
    - `mode`: Current WiFi mode.
    - `apName`: Access Point (AP) name.
    - `apPassword`: Access Point (AP) password.
  - `status`: Object containing current WiFi connection status.

#### POST `/wifi/config`

Updates the WiFi configuration.

**Request Body:**

- `provisioning`: Boolean to enable/disable provisioning (optional).
- `mode`: WiFi mode to set (optional).
- `apName`: Access Point (AP) name to set (optional).
- `apPassword`: Access Point (AP) password to set (optional).

**Response:**

- `200 OK`: Indicates a successful update.
- `400 Bad Request`: Failed to update configuration due to incorrect or missing data.

### 2. WiFi Network List (`/wifi/list`)

#### GET `/wifi/list`

Lists all available WiFi networks.

**Response:**

- `200 OK`: Successful retrieval.
  - Returns a list of available networks, each containing:
    - `ssid`: Network SSID.
    - `signal`: Signal strength.
    - `rate`: Network rate.
    - `in_use`: Boolean indicating if the network is currently in use.
- `400 Bad Request`: Failed to fetch the list of networks.

### 3. WiFi Connect (`/wifi/connect`)

#### POST `/wifi/connect`

Initiates a connection to a specified WiFi network.

**Request Body:**

- `ssid`: SSID of the WiFi network to connect to.
- `password`: Password for the WiFi network.

**Response:**

- `200 OK`: Connection initiated successfully.
- `400 Bad Request`: Failed to initiate connection due to incorrect credentials or other issues.

**Note**: The `POST /wifi/config` endpoint is currently not fully implemented and requires further development for persistence of the configuration changes.

## Notifications

This endpoint facilitates the retrieval and acknowledgment of notifications.

### 1. Get Notifications (`/notifications`)

#### GET `/notifications`

Fetches notifications, with an option to retrieve only unacknowledged ones.

**Query Parameters:**

- `acknowledged`: Set to `"true"` to retrieve only unacknowledged notifications. Defaults to `"false"`.

**Response:**

- `200 OK`: Successful retrieval.
  - Returns an array of notifications, each containing:
    - `id`: Notification ID.
    - `message`: Notification message.
    - `image`: Image URL associated with the notification (if any).
    - `timestamp`: Timestamp of the notification in ISO format.

### 2. Acknowledge Notification (`/notifications/acknowledge`)

#### POST `/notifications/acknowledge`

Acknowledges a specific notification.

**Request Body:**

- `id`: ID of the notification to acknowledge.
- `response`: User's response to the notification (optional).

**Response:**

- `200 OK`: Notification acknowledged successfully.
  - Returns:
    - `status`: "success" indicating successful acknowledgment.

- `404 Not found`: Failure to acknowledge when the notification was not found
  - Returns:
    - `status`: "failure".
    - `message`: Error message, here "Notification not found".

## Settings

The endpoint allows retrieval and updating of application settings.

### Settings (`/settings/{setting_name}`)

#### GET `/settings/{setting_name}`

Fetches the value of a specific setting or all settings if no specific setting name is provided.

**Path Parameters:**

- `setting_name` (optional): Name of the specific setting to retrieve.

**Response:**

- `200 OK`: Successful retrieval.
  - If `setting_name` is provided: Returns the value of the specified setting.
  - If `setting_name` is not provided: Returns all settings.

#### POST `/settings`

Updates the value of a specific setting or multiple settings.

**Request Body:**

- JSON object with multiple keys and values representing the settings to be updated.

**Response:**

- `200 OK`: Successful update.
  - Confirmation message indicating the settings updated.
- `404 Not Found`: One of the provided settings was not found
  - No settings will be persisted in this case

## Update

The endpoint enables firmware updates through the uploading of ZIP files.

### Update Firmware (`/update/firmware`)

#### POST `/update/firmware`

Uploads a ZIP file to update the firmware.

**Request Body:**

- `file`: The ZIP file containing the firmware update. The file should be part of a `multipart/form-data` request.

**Response:**

- `200 OK`: The file is successfully uploaded and unpacked.
  - Returns a confirmation message with the path where the file is unpacked.
- `400 Bad Request`:
  - If no file is uploaded, a message "No file uploaded."
  - If the file is not a ZIP file, a message "Invalid file format. Only ZIP files are accepted."
  - If the uploaded file is not a valid ZIP archive, a message "The uploaded file is not a valid ZIP archive."
- `500 Internal Server Error`: Occurs in case of unexpected errors during file processing.
  - Returns the error message detailing the issue.
