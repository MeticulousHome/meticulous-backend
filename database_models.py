from datetime import datetime
from typing import List, Optional

from sqlalchemy import (
    Float,
    ForeignKey,
    Integer,
    JSON,
    Text,
    DateTime,
)
from sqlalchemy.orm import (
    DeclarativeBase,
    Mapped,
    mapped_column,
    relationship,
)


class Base(DeclarativeBase):
    pass


Base.metadata.naming_convention = {
    "ix": "ix_%(column_0_label)s",
    "uq": "uq_%(table_name)s_%(column_0_name)s",
    "ck": "ck_%(table_name)s_`%(constraint_name)s`",
    "fk": "fk_%(table_name)s_%(column_0_name)s_%(referred_table_name)s",
    "pk": "pk_%(table_name)s",
}


class Profile(Base):
    __tablename__ = "profile"

    key: Mapped[int] = mapped_column(Integer, primary_key=True)
    id: Mapped[str] = mapped_column(Text, nullable=False)
    author: Mapped[Optional[str]] = mapped_column(Text, nullable=True)
    author_id: Mapped[Optional[str]] = mapped_column(Text, nullable=True)
    display: Mapped[Optional[dict]] = mapped_column(JSON, nullable=True)
    final_weight: Mapped[Optional[int]] = mapped_column(Integer, nullable=True)
    last_changed: Mapped[Optional[float]] = mapped_column(Float, nullable=True)
    name: Mapped[Optional[str]] = mapped_column(Text, nullable=True)
    temperature: Mapped[Optional[int]] = mapped_column(Integer, nullable=True)
    stages: Mapped[Optional[list]] = mapped_column(JSON, nullable=True)
    variables: Mapped[Optional[list]] = mapped_column(JSON, nullable=True)
    previous_authors: Mapped[Optional[list]] = mapped_column(JSON, nullable=True)

    histories: Mapped[List["History"]] = relationship(back_populates="profile")


class History(Base):
    __tablename__ = "history"

    id: Mapped[int] = mapped_column(Integer, primary_key=True)
    uuid: Mapped[Optional[str]] = mapped_column(Text, nullable=True)
    file: Mapped[str] = mapped_column(Text, nullable=False)
    time: Mapped[datetime] = mapped_column(DateTime, nullable=False)
    profile_name: Mapped[str] = mapped_column(Text, nullable=False)
    profile_id: Mapped[str] = mapped_column(Text, nullable=False)
    profile_key: Mapped[int] = mapped_column(
        Integer, ForeignKey("profile.key"), nullable=False
    )
    debug_file: Mapped[Optional[str]] = mapped_column(Text, nullable=True)

    profile: Mapped["Profile"] = relationship(back_populates="histories")
    annotation: Mapped[Optional["ShotAnnotation"]] = relationship(
        back_populates="history", uselist=False
    )


class ShotAnnotation(Base):
    __tablename__ = "shot_annotation"

    id: Mapped[int] = mapped_column(Integer, primary_key=True)
    history_id: Mapped[int] = mapped_column(
        Integer, ForeignKey("history.id"), nullable=False, unique=True
    )
    history_uuid: Mapped[str] = mapped_column(
        Text, ForeignKey("history.uuid"), nullable=False, unique=True
    )

    history: Mapped["History"] = relationship(back_populates="annotation")
    rating: Mapped[Optional["ShotRating"]] = relationship(
        back_populates="annotation", uselist=False
    )


class ShotRating(Base):
    __tablename__ = "shot_rating"

    id: Mapped[int] = mapped_column(Integer, primary_key=True)
    annotation_id: Mapped[int] = mapped_column(
        Integer, ForeignKey("shot_annotation.id"), nullable=False, unique=True
    )
    basic: Mapped[Optional[str]] = mapped_column(Text, nullable=True)

    annotation: Mapped["ShotAnnotation"] = relationship(back_populates="rating")


# Backward compatibility exports
metadata = Base.metadata

# FTS structure is defined here for reference
FTS_TABLES = {
    "profile_fts",
    "profile_fts_data",
    "profile_fts_idx",
    "profile_fts_content",
    "profile_fts_docsize",
    "profile_fts_config",
    "stage_fts",
    "stage_fts_data",
    "stage_fts_idx",
    "stage_fts_content",
    "stage_fts_docsize",
    "stage_fts_config",
}
