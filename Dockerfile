FROM python:3.12-bullseye

ENV VIRTUAL_ENV=/opt/venv
RUN python3 -m venv $VIRTUAL_ENV
ENV PATH="$VIRTUAL_ENV/bin:$PATH"

# Install dependencies:
COPY requirements.txt .
RUN pip install -r requirements.txt

WORKDIR /app
# Copy Application files
# FIXME use src-layout
COPY . .

CMD ["python", "back.py"]
