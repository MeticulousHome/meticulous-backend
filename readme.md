# Backend for rasp in esp
## Introduction
This repo is to run the backend of meticulous. 

## Backend: For development

To allow developers to run backend without a physical coffee machine, we implement docker file. Just make the following:

```bash
# Branch
git fetch origin
git switch main

# Docker compose
docker compose run --build -p 8080:8080 backend
```

if you are on linux, just start the backend directly:

```bash
BACKEND=emulator python3 back.py
```

You can interact with the backend using the command line interface after run the docker compose command. For instance, you can enter the commands

```bash
l
```

and

```bash
r
```

to move the dial. These commands will shift the dial to the left or right, respectively.
