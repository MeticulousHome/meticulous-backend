#!/bin/sh
USER_ID=$(id -u) GROUP_ID=$(id -g) docker compose run --build -p 8080:8080 backend /opt/venv/bin/python3 /app/back.py
