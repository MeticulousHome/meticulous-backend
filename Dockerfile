FROM debian:bookworm

RUN apt-get update -y && apt-get dist-upgrade -y
RUN apt-get install -y build-essential libcairo2-dev libxt-dev libgirepository1.0-dev libgstreamer1.0-dev gstreamer1.0-plugins-base-apps bluez \
    libsystemd-dev libparted-dev libglib2.0-dev libffi-dev python3-dev pkg-config \
    python3 \
    python3-setuptools \
    dbus systemd git curl

# Install uv
RUN curl -LsSf https://astral.sh/uv/install.sh | sh
ENV PATH="/root/.local/bin:$PATH"

ENV UV_COMPILE_BYTECODE=1
ENV UV_LINK_MODE=copy
ENV UV_PROJECT_ENVIRONMENT=/opt/venv

WORKDIR /app

# Install dependencies first for better layer caching
COPY pyproject.toml uv.lock ./
RUN uv sync --frozen --no-dev --group machine --no-install-project

COPY UI_timezones.json /usr/share/zoneinfo

RUN mkdir -p /run/dbus
CMD ["/bin/bash", "/app/start_container.sh"]
