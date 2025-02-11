FROM python:3.12-bookworm

RUN apt update
RUN apt-get update -y
RUN apt dist-upgrade -y
RUN apt install -y libcairo2-dev libxt-dev libgirepository1.0-dev libgstreamer1.0-dev bluez
RUN apt-get install -y dbus systemd

ENV PATH=/root/.cargo/bin:$PATH

ENV VIRTUAL_ENV=/opt/venv
RUN python3 -m venv $VIRTUAL_ENV
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