FROM debian:bookworm

RUN apt update
RUN apt-get update -y
RUN apt dist-upgrade -y
RUN apt install -y build-essential libcairo2-dev libxt-dev libgirepository1.0-dev libgstreamer1.0-dev bluez python3 \
    python3-bleak \
    python3-cairo \
    python3-dbus-next \
    python3-gi \
    python3-setuptools \
    python3-systemd \
    python3-venv \
    python3-wheel \
    python3-pip
RUN apt-get install -y dbus systemd git

ENV PATH=/root/.cargo/bin:$PATH

ENV VIRTUAL_ENV=/opt/venv
RUN python3 -m venv --system-site-packages $VIRTUAL_ENV
ENV PATH="$VIRTUAL_ENV/bin:$PATH"

RUN /opt/venv/bin/pip install --upgrade pip
RUN /opt/venv/bin/pip install --upgrade setuptools
RUN /opt/venv/bin/pip install --upgrade wheel

# Install dependencies:
COPY requirements.txt .
COPY UI_timezones.json /usr/share/zoneinfo
RUN /opt/venv/bin/pip install -r requirements.txt

WORKDIR /app
# Copy Application files
# FIXME use src-layout
COPY . .

RUN mkdir /run/dbus
RUN chmod +x /app/start_container.sh

CMD ["/bin/bash", "/app/start_container.sh"]
