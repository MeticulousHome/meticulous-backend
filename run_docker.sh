#!/bin/sh
docker compose run --build -p 8080:8080 backend python3 /app/back.py

