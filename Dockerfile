FROM python:3.13-slim

# Устанавливаем зависимости, необходимые для Pydub и компиляции
RUN apt-get update && apt-get install -y \
    gcc \
    libasound2 \
    libasound2-dev \
    build-essential \
    && rm -rf /var/lib/apt/lists/*

# Создаем рабочую директорию
WORKDIR /app

# Копируем файлы зависимостей
COPY requirements.txt ./

# Устанавливаем зависимости
RUN pip install --no-cache-dir -r requirements.txt

# Копируем файлы приложения
COPY config.py handlers.py main.py utils.py database.py watchdog_runner.py ./
COPY tests/ ./tests/

# Устанавливаем переменные окружения
ENV PYTHONUNBUFFERED=1

# Запускаем скрипт
CMD ["python", "watchdog_runner.py"]
