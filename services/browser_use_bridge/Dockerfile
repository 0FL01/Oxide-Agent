FROM python:3.12-slim

ENV PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    PIP_DISABLE_PIP_VERSION_CHECK=1 \
    BROWSER_USE_BRIDGE_HOST=0.0.0.0 \
    BROWSER_USE_BRIDGE_PORT=8000 \
    BROWSER_USE_BRIDGE_DATA_DIR=/data \
    BROWSER_USE_HOME=/data

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        chromium \
        curl \
        fonts-dejavu-core \
        fonts-freefont-ttf \
        fonts-liberation \
        fonts-noto-core \
        fonts-unifont \
    && rm -rf /var/lib/apt/lists/* \
    && ln -sf /usr/bin/chromium /usr/bin/chromium-browser

RUN useradd --system --create-home --shell /bin/bash browseruse \
    && mkdir -p /app /data \
    && chown -R browseruse:browseruse /app /data /home/browseruse

WORKDIR /app

COPY requirements.txt ./requirements.txt
RUN pip install --no-cache-dir -r requirements.txt

COPY app ./app

USER browseruse

EXPOSE 8000
VOLUME ["/data"]

CMD ["uvicorn", "app.main:app", "--host", "0.0.0.0", "--port", "8000"]
