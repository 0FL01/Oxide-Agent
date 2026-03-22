# Production Checklist

> **Чек-лист для деплоя в продакшен**
>
> 📁 **Раздел:** Deployment
> 🎯 **Цель:** Успешный деплой

---

## 📋 Оглавление

- [Перед деплоем](#перед-деплоем)
- [Конфигурация](#конфигурация)
- [Opencode](#opencode)
- [Sandbox](#sandbox)
- [Мониторинг](#мониторинг)
- [Безопасность](#безопасность)
- [После деплоя](#после-деплоя)

---

## Перед деплоем

### Проверка кода

- [ ] Код протестирован локально
- [ ] Unit тесты проходят: `cargo test`
- [ ] Интеграционные тесты проходят: `cargo test --ignored`
- [ ] Linting прошел: `cargo clippy`
- [ ] Formatting проверен: `cargo fmt --check`
- [ ] Нет TODO/FIXME в продакшен коде
- [ ] Документация обновлена

### Проверка зависимостей

- [ ] Все зависимости из надежных источников
- [ ] Нет уязвимых зависимостей: `cargo audit`
- [ ] Версии зависимостей зафиксированы (Cargo.lock)
- [ ] Нет dev зависимостей в prod builds

### Проверка конфигурации

- [ ] Переменные окружения настроены
- [ ] Secrets зашифрованы
- [ ] Параметры оптимизированы для продакшена
- [ ] Логи отключены в критических секциях (не для production!)

---

## Конфигурация

### Переменные окружения

```bash
#!/bin/bash
# prod.env - Продакшен конфигурация

# Opencode
export OPENCODE_BASE_URL=http://opencode-internal:4096
export OPENCODE_TIMEOUT=600

# Sandbox
export SANBOX_DOCKER_IMAGE=company/agent-sandbox:prod
export SANBOX_MEMORY_LIMIT=1
export SANBOX_CPU_LIMIT=2

# LLM
export LLM_MODEL=anthropic/claude-3-opus
export LLM_TEMPERATURE=0.2
export LLM_MAX_TOKENS=8000

# Логирование
export RUST_LOG=info
export LOG_FILE=/var/log/agent.log

# Безопасность
export API_KEY=***ENCRYPTED***
export DB_PASSWORD=***ENCRYPTED***
```

### Проверка переменных

- [ ] OPENCODE_BASE_URL настроен
- [ ] OPENCODE_TIMEOUT достаточно большой
- [ ] SANBOX_DOCKER_IMAGE тегged version
- [ ] SANBOX_MEMORY_LIMIT подходящий
- [ ] SANBOX_CPU_LIMIT не превышает доступные ресурсы
- [ ] LLM_TEMPERATURE достаточно низкий (0.2-0.3)
- [ ] LLM_MAX_TOKENS оптимизирован
- [ ] RUST_LOG установлен в info или warn
- [ ] LOG_FILE доступен и имеет место на диске

---

## Opencode

### Установка

- [ ] Opencode установлен на всех нодах
- [ ] Version совпадает (используйте фиксированную версию)
- [ ] Docker образ Opencode доступен
- [ ] Порты открыты в firewall: `4096/tcp`

### Конфигурация

- [ ] Архитектор agent создан (`.opencode/agent/architect.md`)
- [ ] Permissions настроены правильно
- [ ] Git настроен: `git config --global`
- [ ] Репозиторий инициализирован
- [ ] Ветка production используется

### Запуск

- [ ] Opencode запускается через systemd/supervisord
- [ ] Auto-restart настроен
- [ ] Health check endpoint доступен: `/vcs`
- [ ] API документация доступна: `/doc`
- [ ] Логи пишутся в syslog

### Проверка

```bash
# Проверить, что сервер запущен
curl http://opencode-internal:4096/vcs

# Проверить health endpoint
curl http://opencode-internal:4096/health

# Проверить статус процесса
systemctl status opencode

# Проверить логи
journalctl -u opencode -f
```

---

## Sandbox

### Docker образ

- [ ] Docker образ построен
- [ ] Образ tagged: `company/agent-sandbox:prod`
- [ ] Образ оптимизирован: `docker build --squash`
- [ ] Обновлен: включает Python, yt-dlp, ffmpeg
- [ ] Без уязвимостей: `docker scan`

### Контейнеры

- [ ] Resource limits настроены (CPU, memory)
- [ ] Volume mounts настроены
- [ ] Network изоляция включена
- [ ] Restart policy настроена
- [ ] Health check настроен

### Конфигурация

```yaml
# docker-compose.yml
version: "3.8"

services:
  agent-sandbox:
    image: company/agent-sandbox:prod
    container_name: agent-sandbox-prod
    restart: unless-stopped
    mem_limit: 1g
    cpus: "2"
    volumes:
      - sandbox-workspace:/workspace
    healthcheck:
      test: ["CMD", "test", "-f", "/workspace"]
      interval: 30s
      timeout: 10s
      retries: 3
    security_opt:
      - no-new-privileges:true
    networks:
      - agent-network

volumes:
  sandbox-workspace:

networks:
  agent-network:
    driver: bridge
```

### Проверка

```bash
# Проверить, что контейнеры запущены
docker ps | grep agent-sandbox

# Проверить ресурсы
docker stats agent-sandbox-prod

# Проверить логи
docker logs -f agent-sandbox-prod

# Проверить health
docker inspect agent-sandbox-prod | grep -A 10 Health
```

---

## Мониторинг

### Логи

- [ ] Логи настроены в `/var/log/agent/`
- [ ] Log rotation настроена (logrotate)
- [ ] Logs отправляются в centralized logging (ELK, CloudWatch)
- [ ] Error alerts настроены
- [ ] Performance metrics собираются

### Метрики

- [ ] Prometheus exporter запущен
- [ ] Grafana дашборды настроены
- [ ] Metrics: request rate, latency, errors
- [ ] Alerts: high latency, high error rate
- [ ] Resource metrics: CPU, memory, disk

### Health Checks

- [ ] `/health` endpoint доступен
- [ ] Health check запускается каждые 30s
- [ ] Проверяется Opencode доступность
- [ ] Проверяется Docker контейнер
- [ ] Alert на health check failure

---

## Безопасность

### Secrets

- [ ] Secrets не в коде
- [ ] Secrets в Vault или AWS Secrets Manager
- [ ] Environment variables зашифрованы
- [ ] No hardcoded passwords
- [ ] API keys rotated регулярно

### Network

- [ ] HTTPS везде (кроме internal)
- [ ] Firewall правила настроены
- [ ] Only necessary ports open
- [ ] Internal services не доступны извне
- [ ] TLS/SSL сертификаты валидны

### Docker

- [ ] Running as non-root user
- [ ] No privileged containers
- [ ] Read-only filesystem где возможно
- [ ] Seccomp profiles включены
- [ ] AppArmor/SELinux включен

### Git

- [ ] Git hooks настроены (pre-commit, pre-push)
- [ ] Signed commits включены
- [ ] Branch protection включен
- [ ] Required reviewers настроены
- [ ] Automated tests на PRs

---

## После деплоя

### Smoke tests

- [ ] Базовые операции работают:
  - [ ] Sandbox: `execute_command` с простыми командами
  - [ ] Opencode: создать session
  - [ ] Opencode: отправить prompt
- [ ] Пример задачи выполняется:
  - [ ] Скачивание файла работает
  - [ ] Добавление простой функции работает

### Validation

- [ ] Логи без ошибок
- [ ] Метрики в нормальном диапазоне
- [ ] Alerts не срабатывают
- [ ] Производительность в ожидаемом диапазоне

### Documentation

- [ ] Runbook обновлен
- [ ] On-call procedures задокументированы
- [ ] Troubleshooting guide обновлен
- [ ] Team уведомлен о деплое

---

## Rollback Plan

### Триггеры

- [ ] Error rate > 5%
- [ ] Latency > 2x baseline
- [ ] Health checks failing > 3 times
- [ ] Critical bugs найдены

### Процедура

```bash
#!/bin/bash
# rollback.sh - Rollback procedure

echo "Starting rollback..."

# 1. Stop new version
systemctl stop agent

# 2. Switch to previous version
git checkout <previous-commit>

# 3. Build and deploy
cargo build --release
systemctl start agent

# 4. Verify
sleep 10
curl http://localhost:4096/vcs || {
  echo "Rollback failed!"
  exit 1
}

echo "Rollback successful!"
```

### Проверки

- [ ] Rollback задокументирован
- [ ] Rollback протестирован
- [ ] Team знает как сделать rollback
- [ ] Automated rollback настроен (если возможно)

---

## Проверочный список

### Перед нажатием "Deploy"

- [ ] Все тесты зеленые
- [ ] Code review завершен
- [ ] Документация обновлена
- [ ] Secrets настроены
- [ ] Health checks работают
- [ ] Rollback план готов
- [ ] Team уведомлен
- [ ] Maintenance window согласован
- [ ] Backup текущей версии сделан

### После деплоя

- [ ] Smoke tests пройдены
- [ ] Метрики в норме
- [ ] Нет alert
- [ ] Users сообщают об успехе
- [ ] Documentation обновлена
- [ ] Runbook обновлен

---

## Следующие шаги

- [ ] Деплой в staging
- [ ] UAT с реальными пользователями
- [ ] Финальный деплой в продакшен
- [ ] Post-deploy monitoring
- [ ] Retrospective

---

**Связанные документы:**

- [deployment/monitoring.md](./monitoring.md) - Мониторинг и логирование
- [testing/troubleshooting.md](../testing/troubleshooting.md) - Устранение проблем
- [configuration/environment_variables.md](../configuration/environment_variables.md) - Переменные окружения
