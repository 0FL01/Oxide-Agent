#!/bin/sh

set -e

# Проверка необходимых переменных
if [ -z "$SSH_PORT" ] || [ -z "$SSH_USERNAME" ] || [ -z "$SSH_HOST" ] || [ -z "$CI_COMMIT_SHORT_SHA" ] || [ -z "$SERVICE_DIR" ] || [ -z "$DOCKER_IMAGE" ]; then
  echo "Ошибка: Не установлена одна или несколько обязательных переменных окружения (SSH_PORT, SSH_USERNAME, SSH_HOST, CI_COMMIT_SHORT_SHA, SERVICE_DIR, DOCKER_IMAGE)."
  exit 1
fi
if [ -z "$GROQ_API_KEY" ] || [ -z "$TELEGRAM_TOKEN" ] || [ -z "$MISTRAL_API_KEY" ] || [ -z "$GEMINI_API_KEY" ] || [ -z "$ADMIN_ID" ] || [ -z "$POSTGRES_DB" ] || [ -z "$POSTGRES_USER" ] || [ -z "$POSTGRES_PASSWORD" ] || [ -z "$POSTGRES_HOST" ] || [ -z "$POSTGRES_PORT" ]; then
   echo "Ошибка: Не установлена одна или несколько переменных для .env файла."
   exit 1
fi

# Используем переменную GitLab CI CI_COMMIT_SHORT_SHA вместо самодельной SHA_SHORT
echo "Подключение к $SSH_USERNAME@$SSH_HOST:$SSH_PORT"
echo "Развертывание образа ${DOCKER_IMAGE}:${CI_COMMIT_SHORT_SHA} в ${SERVICE_DIR}"

# Вся команда выполняется удаленно через SSH
ssh -p "$SSH_PORT" "$SSH_USERNAME@$SSH_HOST" "
  set -e
  echo '--- Начало удаленного развертывания ---'

  # Передаем нужные переменные внутрь удаленной сессии явно
  # Обратите внимание на экранирование $ для переменных, которые должны раскрыться локально
  # и отсутствие экранирования для тех, что должны раскрыться удаленно (но мы передаем их значения)
  export GROQ_API_KEY='${GROQ_API_KEY}'
  export TELEGRAM_TOKEN='${TELEGRAM_TOKEN}'
  export MISTRAL_API_KEY='${MISTRAL_API_KEY}'
  export GEMINI_API_KEY='${GEMINI_API_KEY}'
  export ADMIN_ID='${ADMIN_ID}'
  export POSTGRES_DB='${POSTGRES_DB}'
  export POSTGRES_USER='${POSTGRES_USER}'
  export POSTGRES_PASSWORD='${POSTGRES_PASSWORD}'
  export POSTGRES_HOST='${POSTGRES_HOST}'
  export POSTGRES_PORT='${POSTGRES_PORT}'
  export DOCKER_IMAGE='${DOCKER_IMAGE}'
  export CI_COMMIT_SHORT_SHA='${CI_COMMIT_SHORT_SHA}'
  export SERVICE_DIR='${SERVICE_DIR}'

  if [ -z \"\${CI_COMMIT_SHORT_SHA}\" ]; then echo 'Ошибка: CI_COMMIT_SHORT_SHA не определена на удаленном хосте'; exit 1; fi
  echo \"Создание директории сервиса: \${SERVICE_DIR}\"
  mkdir -p \${SERVICE_DIR} && cd \${SERVICE_DIR}

  echo 'Создание файла .env'
  # Используем EOF_ENV без кавычек для подстановки переменных
  cat << EOF_ENV > .env
GROQ_API_KEY=\${GROQ_API_KEY}
TELEGRAM_TOKEN=\${TELEGRAM_TOKEN}
MISTRAL_API_KEY=\${MISTRAL_API_KEY}
GEMINI_API_KEY=\${GEMINI_API_KEY}
ADMIN_ID=\${ADMIN_ID}
POSTGRES_DB=\${POSTGRES_DB}
POSTGRES_USER=\${POSTGRES_USER}
POSTGRES_PASSWORD=\${POSTGRES_PASSWORD}
POSTGRES_HOST=\${POSTGRES_HOST}
POSTGRES_PORT=\${POSTGRES_PORT}
EOF_ENV
  echo 'Файл .env создан'

  echo 'Создание файла docker-compose.yml'
  # Используем EOF_COMPOSE без кавычек
  cat << EOF_COMPOSE > docker-compose.yml
services:
  another_chat_tg:
    image: \${DOCKER_IMAGE}:\${CI_COMMIT_SHORT_SHA}
    container_name: another_chat_tg
    network_mode: \"host\"
    environment:
      - POSTGRES_HOST=127.0.0.1
    restart: unless-stopped
    volumes:
      - ./.env:/app/.env:ro
EOF_COMPOSE
  echo 'Файл docker-compose.yml создан'

  echo 'Загрузка нового образа...'
  docker compose pull another_chat_tg

  echo 'Остановка и удаление старого контейнера (если существует)...'
  docker compose down || true

  echo 'Запуск нового контейнера...'
  docker compose up -d

  echo 'Проверка запущенных контейнеров...'
  docker compose ps

  echo 'Очистка старых образов...'
  # Экранируем $ в awk '{ print \$2 }'
  docker images --format \"{{.Repository}}:{{.Tag}} {{.ID}}\" | grep \"^\${DOCKER_IMAGE}\" | grep -v \":\${CI_COMMIT_SHORT_SHA}\" | awk '{ print \$2 }' | xargs -r docker rmi -f || true
  echo '--- Удаленное развертывание завершено ---'
" # Эта кавычка закрывает всю строку команды ssh

echo "Скрипт deploy.sh завершен локально."