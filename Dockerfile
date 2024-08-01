# Используем официальный образ Python 3.10 в качестве базового образа
FROM python:3.10-slim

# Устанавливаем зависимости, необходимые для Pydub
RUN apt-get update && apt-get install libasound2-dev gcc -y

# Создаем рабочую директорию
WORKDIR /app

# Копируем файлы в контейнер
COPY config.py handlers.py main.py utils.py watchdog_runner.py allowed_users.txt requirements.txt ./

# Устанавливаем зависимости
RUN pip install --upgrade pip 
RUN pip install -r requirements.txt

# Устанавливаем переменные окружения для корректной работы aiogram
ENV PYTHONUNBUFFERED=1

# Запускаем скрипт
CMD ["python", "watchdog_runner.py"]
