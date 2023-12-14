
### [v0.1.6] - 2023-12-14
The existing logging logic manually rotates the logs on application start. The old logs are appended and the new logs go into a new file. The maximum amount of logs was 99999 files.


### [v0.1.5] - 2023-09-27
In this pull request, significant enhancements have been made to support the reception of simplified JSON data from Dashboard 1.0 and Italian 1.0 platforms. The system now seamlessly handles simplified JSON data ensuring correct parsing and processing of the incoming data. Additionally, compatibility has been narrowed down exclusively to VAR-SOM, optimizing the codebase and ensuring reliable performance on the said platform. Moreover, communication has been established with an external watcher service, whereby the system now sends notifications to inform the watcher service of its active status, enhancing operational monitoring and ensuring uninterrupted operation. These updates collectively bolster interoperability with Dashboard 1.0 and Italian 1.0, streamline operation on VAR-SOM, and improve monitoring capabilities through watcher service communication.


### [v0.1.4] - 2023-05-19
Added workflow to generate releases

