#!/bin/sh

set -e

# Проверка необходимых переменных
if [ -z "$SSH_PORT" ] || [ -z "$SSH_USERNAME" ] || [ -z "$SSH_HOST" ] || [ -z "$CI_COMMIT_SHORT_SHA" ] || [ -z "$SERVICE_DIR" ] || [ -z "$DOCKER_IMAGE" ]; then
  echo "Ошибка: Не установлена одна или несколько обязательных переменных окружения (SSH_PORT, SSH_USERNAME, SSH_HOST, CI_COMMIT_SHORT_SHA, SERVICE_DIR, DOCKER_IMAGE)."
  exit 1
fi
if [ -z "$GROQ_API_KEY" ] || [ -z "$TELEGRAM_TOKEN" ] || [ -z "$MISTRAL_API_KEY" ] || [ -z "$GEMINI_API_KEY" ] || [ -z "$ADMIN_ID" ] || [ -z "$R2_ACCESS_KEY_ID" ] || [ -z "$R2_SECRET_ACCESS_KEY" ] || [ -z "$R2_ENDPOINT_URL" ] || [ -z "$R2_BUCKET_NAME" ]; then
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
  export GROQ_API_KEY='${GROQ_API_KEY}'
  export TELEGRAM_TOKEN='${TELEGRAM_TOKEN}'
  export MISTRAL_API_KEY='${MISTRAL_API_KEY}'
  export GEMINI_API_KEY='${GEMINI_API_KEY}'
  export ADMIN_ID='${ADMIN_ID}'
  export R2_ACCESS_KEY_ID='${R2_ACCESS_KEY_ID}'
  export R2_SECRET_ACCESS_KEY='${R2_SECRET_ACCESS_KEY}'
  export R2_ENDPOINT_URL='${R2_ENDPOINT_URL}'
  export R2_BUCKET_NAME='${R2_BUCKET_NAME}'

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
R2_ACCESS_KEY_ID=\${R2_ACCESS_KEY_ID}
R2_SECRET_ACCESS_KEY=\${R2_SECRET_ACCESS_KEY}
R2_ENDPOINT_URL=\${R2_ENDPOINT_URL}
R2_BUCKET_NAME=\${R2_BUCKET_NAME}

EOF_ENV
  echo 'Файл .env создан'

  echo 'Создание файла docker-compose.yml'
  # Используем EOF_COMPOSE без кавычек
  cat << EOF_COMPOSE > docker-compose.yml
services:
  another_chat_tg:
    image: \${DOCKER_IMAGE}:\${CI_COMMIT_SHORT_SHA}
    container_name: another_chat_tg
    network_mode: \"bridge\"
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