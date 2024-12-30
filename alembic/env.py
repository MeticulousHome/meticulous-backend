from logging.config import fileConfig
from sqlalchemy import (
    engine_from_config,
    MetaData,
    Table,
    Column,
    Integer,
    Text,
    DateTime,
    JSON,
    ForeignKey,
    Float,
    BLOB
)
from sqlalchemy import pool
from alembic import context

# Esto es el objeto de configuración de Alembic
config = context.config

# Configurar logging
if config.config_file_name is not None:
    fileConfig(config.config_file_name)

# Crear dos objetos MetaData separados
metadata = MetaData()  # Para tablas principales
fts_metadata = MetaData()  # Para tablas FTS (no se usará en migraciones)

# Definir las tablas principales
profile = Table(
    'profile', metadata,
    Column('key', Integer, primary_key=True),
    Column('id', Text, nullable=False),
    Column('author', Text),
    Column('author_id', Text),
    Column('display', JSON),
    Column('final_weight', Integer),
    Column('last_changed', Float),
    Column('name', Text),
    Column('temperature', Integer),
    Column('stages', JSON),
    Column('variables', JSON),
    Column('previous_authors', JSON)
)

history = Table(
    'history', metadata,
    Column('id', Integer, primary_key=True),
    Column('uuid', Text),
    Column('file', Text, nullable=False),
    Column('time', DateTime, nullable=False),
    Column('profile_name', Text, nullable=False),
    Column('profile_id', Text, nullable=False),
    Column('profile_key', Integer, ForeignKey('profile.key'), nullable=False)
)

# Asignar el metadata al target_metadata
target_metadata = metadata

def include_object(object, name, type_, reflected, compare_to):
    # Lista de todas las tablas relacionadas con FTS que queremos excluir
    fts_tables = {
        'profile_fts', 'profile_fts_data', 'profile_fts_idx', 
        'profile_fts_content', 'profile_fts_docsize', 'profile_fts_config',
        'stage_fts', 'stage_fts_data', 'stage_fts_idx', 
        'stage_fts_content', 'stage_fts_docsize', 'stage_fts_config'
    }
    
    if type_ == "table":
        # Excluir tablas FTS específicas y tablas SQLite internas
        if name in fts_tables or name.startswith('sqlite_'):
            return False
        return True
    return True

def run_migrations_offline() -> None:
    """Run migrations in 'offline' mode.
    
    This configures the context with just a URL
    and not an Engine, though an Engine is acceptable
    here as well.  By skipping the Engine creation
    we don't even need a DBAPI to be available.
    """
    url = config.get_main_option("sqlalchemy.url")
    context.configure(
        url=url,
        target_metadata=target_metadata,
        literal_binds=True,
        dialect_opts={"paramstyle": "named"},
        include_object=include_object,
    )

    with context.begin_transaction():
        context.run_migrations()

def run_migrations_online() -> None:
    """Run migrations in 'online' mode.
    
    In this scenario we need to create an Engine
    and associate a connection with the context.
    """
    connectable = engine_from_config(
        config.get_section(config.config_ini_section),
        prefix="sqlalchemy.",
        poolclass=pool.NullPool,
    )

    with connectable.connect() as connection:
        context.configure(
            connection=connection,
            target_metadata=target_metadata,
            include_object=include_object,
            render_as_batch=True,  # Necesario para SQLite
            compare_type=False,  # Desactivar comparación de tipos para evitar cambios TEXT/String
            compare_server_default=True
        )

        with context.begin_transaction():
            context.run_migrations()

if context.is_offline_mode():
    run_migrations_offline()
else:
    run_migrations_online()