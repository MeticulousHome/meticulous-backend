# meticulous-backend

## Introduction

This repository is used to run the backend of **meticulous**.
It handles ESP32 serial communication, shot management, profiles, sounds, OTA updates, and exposes a REST + Socket.IO API.

## Dependencies

This project uses [uv](https://docs.astral.sh/uv/) for dependency management. Dependencies are declared in `pyproject.toml` and pinned in `uv.lock`.

### Install uv

```bash
curl -LsSf https://astral.sh/uv/install.sh | sh
```

### Install dependencies

```bash
uv sync
```

This installs the base dependencies into a `.venv` managed by uv.

#### Dependency groups

Some dependencies require system C libraries that are only available on the target machine or in the Docker build environment:

```bash
# On the target machine (installs gpiod, PyGObject, pycairo, pyparted)
uv sync --group machine
```

For development tools (black, flake8, pre-commit, pytest):

```bash
uv sync --group dev
```

### Adding or updating dependencies

```bash
# Add a new dependency
uv add <package>

# Add a dev dependency
uv add --group dev <package>

# Update the lock file after manual edits to pyproject.toml
uv lock
```

## Code style

**NOTE: As we are running the backend on a debian `bookworm` system, the installed `python3` binary is set automatically set to `3.11.2`. In order to avoid possible syntax issues or feature missmatches, install the aforementioned version in the system you develop in, preferably in a virtual environment, its possible to make use of the `pyenv` tool to help with the installation and management of multiple python versions**

- Formatter: **black** (line-length 96)
- Linter: **flake8**
- Pre-commit hooks are configured in `.pre-commit-config.yaml`

```bash
# Install pre-commit hooks
uv run pre-commit install

# Run manually
uv run pre-commit run --all-files
```

## Backend: For Development

To allow developers to run the backend without a physical coffee machine, we have implemented a Docker configuration. Follow these steps:

### Using Docker

```bash
# Branch
git fetch origin
git switch main

# Docker compose
docker compose run --build -p 8080:8080 backend
```

### Running Directly on Linux

If you are on Linux, start the backend directly using the emulation script:

```bash
./run_emulated.sh
```

This sets up environment variables for local development and runs the backend with `uv run`.

### Running arbitrary commands

Use `uv run` to execute commands within the managed virtualenv:

```bash
uv run python3 back.py
uv run pytest
uv run black --check .
uv run flake8
```

You can interact with the backend using the command line interface. For instance, you can enter the commands `l` and `r` to move the dial left or right, respectively.

## Database Migrations Guide

This project uses Alembic for managing database migrations. Follow these steps to handle any changes in the database structure:

### Making Database Changes

1. **Modify Database Models:**
   Edit `database_models.py` to reflect the required changes in your database structure. You can:
   - Add or modify tables, columns, or constraints.
   - Remember, this file is the single source of truth for the database schema.

2. **Generate a Migration Script:**
   Run the following command to create a new migration script:
   ```bash
   uv run alembic revision --autogenerate -m "Brief description of change"
   ```
   - A new script will be generated in the `alembic/versions` directory.
   - Open the generated script and review the `upgrade()` and `downgrade()` functions.
   - Ensure these functions accurately reflect your intended changes, and modify them if necessary.

3. **Apply the Migration:**
   Update your local database to the latest version with:
   ```bash
   uv run alembic upgrade head
   ```

### Deployment

- Simply push your changes to the `main` branch.
- Other machines will automatically apply the migrations using the `db_migration_updater.py` script.

### Rolling Back Changes

If you need to revert to a previous database version:

1. **Identify the Revision:**
   Find the desired revision ID from the scripts in the `alembic/versions` directory.

2. **Set the Stable Version:**
   Update the version in `db_migration_updater.py`:
   ```python
   MIGRATION_VERSION_STABLE = "revision_id"  # e.g., "ebb6a77afd0e"
   ```

3. **Automatic Downgrade:**
   The system will automatically downgrade the database to the specified version.

## Additional Notes

- **Testing:** Always test your migration scripts locally before deploying them.
- **Collaboration:** Communicate with your team when making significant changes to the database schema.
- **Documentation:** Keep your migration messages clear to track the evolution of the database.
