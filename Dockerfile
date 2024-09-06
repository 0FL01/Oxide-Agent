# Используем официальный образ Python 3.10 Alpine в качестве базового образа
FROM python:3.10-alpine

# Устанавливаем зависимости, необходимые для Pydub и компиляции, без сохранения кэша
RUN apk add --no-cache \
    gcc \
    musl-dev \
    alsa-lib \
    alsa-lib-dev \
    build-base

# Создаем рабочую директорию
WORKDIR /app

# Копируем файлы в контейнер
COPY config.py handlers.py main.py utils.py watchdog_runner.py allowed_users.txt requirements.txt ./

# Обновляем pip и устанавливаем зависимости без сохранения кэша
RUN pip install --upgrade pip \
    && pip install --no-cache-dir -r requirements.txt

# Устанавливаем переменные окружения для корректной работы aiogram
ENV PYTHONUNBUFFERED=1

# Запускаем скрипт
CMD ["python", "watchdog_runner.py"]

