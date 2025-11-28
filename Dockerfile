# Стадия сборки
FROM python:3.13-slim AS builder

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

# Стадия выполнения
FROM python:3.13-slim

# Устанавливаем зависимости, необходимые для Pydub в рантайме
RUN apt-get update && apt-get install -y \
    libasound2 \
    && rm -rf /var/lib/apt/lists/*

# Создаем рабочую директорию
WORKDIR /app

# Копируем зависимости из стадии сборки
COPY --from=builder /usr/local/lib/python3.13/site-packages /usr/local/lib/python3.13/site-packages

# Копируем файлы приложения
COPY config.py handlers.py main.py utils.py database.py watchdog_runner.py ./

# Устанавливаем переменные окружения для корректной работы aiogram
ENV PYTHONUNBUFFERED=1

# Запускаем скрипт
CMD ["python", "watchdog_runner.py"]
