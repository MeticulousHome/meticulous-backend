FROM python:3.12-bullseye

RUN apt update
RUN apt dist-upgrade -y
RUN apt install -y libcairo2-dev libxt-dev libgirepository1.0-dev libgstreamer1.0-dev

ENV PATH=/root/.cargo/bin:$PATH

ENV VIRTUAL_ENV=/opt/venv
RUN python3 -m venv $VIRTUAL_ENV
ENV PATH="$VIRTUAL_ENV/bin:$PATH"

RUN /opt/venv/bin/pip install --upgrade pip
RUN /opt/venv/bin/pip install --upgrade setuptools
RUN /opt/venv/bin/pip install --upgrade wheel

# Install dependencies:
COPY requirements.txt .
RUN /opt/venv/bin/pip install -r requirements.txt

WORKDIR /app
# Copy Application files
# FIXME use src-layout
COPY . .

CMD ["/opt/venv/bin/python3", "back.py"]
