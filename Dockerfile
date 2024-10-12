# Используем официальный образ Python 3.10 slim в качестве базового образа
FROM python:3.10-slim

# Устанавливаем зависимости, необходимые для Pydub и компиляции
RUN apt-get update && apt-get install -y \
    gcc \
    libasound2 \
    libasound2-dev \
    build-essential \
    && rm -rf /var/lib/apt/lists/*

# Создаем рабочую директорию
WORKDIR /app

# Копируем файлы в контейнер
COPY config.py handlers.py main.py utils.py watchdog_runner.py allowed_users.txt requirements.txt ./

# Обновляем pip и устанавливаем зависимости
RUN pip install --no-cache-dir --upgrade pip \
    && pip install --no-cache-dir -r requirements.txt

# Устанавливаем переменные окружения для корректной работы aiogram
ENV PYTHONUNBUFFERED=1

# Запускаем скрипт
CMD ["python", "watchdog_runner.py"]
