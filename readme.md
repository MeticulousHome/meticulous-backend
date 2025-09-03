# meticulous-backend
## Introduction
This repository is used to run the backend of **meticulous**.

## Install development tools

To keep code format uniform, we make use of `black` formatter and `flake8` linter. You can install and run them
manually or automate that by using the pre-commit hooks that are available in the repo, for that You must install the requirements listed in `requirements-dev.txt`.

```bash
pip install -r requirements-dev.txt
```

Once the requirements are installed, install the pre-commit script using the pre-commit module


```bash
pre-commit install
```

And with that You are good to go. `blake` and `flake8` will be run on every commit attempt

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
   alembic revision --autogenerate -m "Brief description of change"
   ```
   - A new script will be generated in the `alembic/versions` directory.
   - Open the generated script and review the `upgrade()` and `downgrade()` functions.
   - Ensure these functions accurately reflect your intended changes, and modify them if necessary.

3. **Apply the Migration:**  
   Update your local database to the latest version with:
   ```bash
   alembic upgrade head
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
