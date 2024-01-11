
### [v0.1.17] - 2024-01-11
    The meticulous machine is currently only reachable if the use


### [v0.1.16] - 2024-01-09
Some small cleanups to make our life easier


### [v0.1.15] - 2024-01-05
    So far the backend is not persisting any data. Instead it is running completely


### [v0.1.14] - 2024-01-05
To allow the frontend team to develop against the backend without the neeed for a real machine we are building a standalone backend running in a docker container so that the websocket connection can be used with this dummy data. As a first draft the container is running in host networking mode to allow for easy connection to localhost.


### [v0.1.13] - 2024-01-05
The ESP32 connection requires some sensors to be connected to be usefull.


### [v0.1.12] - 2024-01-05
null


### [v0.1.11] - 2024-01-05
We are currently parsing all communication from the ESP32 in back.py and are reconstructing SIO response data from it again later. Move this logic into its own read-only data-classes to ensure proper transport and clean up the parsing code


### [v0.1.10] - 2023-12-26
To provision the machines wifi we need a way to get the configuration


### [v0.1.9] - 2023-12-26
So far the ESP32's connection-handling lived inside the back.py main file.


### [v0.1.8] - 2023-12-15
While we might have fixed the loglevel for the main thread, creating a basic configuration can lead to problematic side- effects on certain setups.


### [v0.1.7] - 2023-12-15
The recently added log modules is not working relyable across different python versions when the loglevel is set.


### [v0.1.6] - 2023-12-14
The existing logging logic manually rotates the logs on application start. The old logs are appended and the new logs go into a new file. The maximum amount of logs was 99999 files.


### [v0.1.5] - 2023-09-27
In this pull request, significant enhancements have been made to support the reception of simplified JSON data from Dashboard 1.0 and Italian 1.0 platforms. The system now seamlessly handles simplified JSON data ensuring correct parsing and processing of the incoming data. Additionally, compatibility has been narrowed down exclusively to VAR-SOM, optimizing the codebase and ensuring reliable performance on the said platform. Moreover, communication has been established with an external watcher service, whereby the system now sends notifications to inform the watcher service of its active status, enhancing operational monitoring and ensuring uninterrupted operation. These updates collectively bolster interoperability with Dashboard 1.0 and Italian 1.0, streamline operation on VAR-SOM, and improve monitoring capabilities through watcher service communication.


### [v0.1.4] - 2023-05-19
Added workflow to generate releases

