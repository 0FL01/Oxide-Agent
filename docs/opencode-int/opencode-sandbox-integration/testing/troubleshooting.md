# Troubleshooting

> **Устранение проблем и FAQ**
>
> 📁 **Раздел:** Testing
> 🎯 **Цель:** Решить возникающие проблемы

---

## 📋 Оглавление

- [Общие проблемы](#общие-проблемы)
- [Opencode проблемы](#opencode-проблемы)
- [Sandbox проблемы](#sandbox-проблемы)
- [LLM проблемы](#llm-проблемы)
- [Сетевые проблемы](#сетевые-проблемы)

---

## Общие проблемы

### Проблема: Integration не работает

**Симптомы:**

- Никакие инструменты не работают
- Ошибки connection refused

**Решение:**

1. Проверьте, что Opencode сервер запущен:

```bash
curl http://127.0.0.1:4096/vcs
```

2. Проверьте Docker контейнер:

```bash
docker ps | grep agent-sandbox
```

3. Проверьте переменные окружения:

```bash
echo $OPENCODE_BASE_URL
echo $OPENCODE_TIMEOUT
```

---

### Проблема: Медленная работа

**Симптомы:**

- Задачи выполняются очень долго
- Timeout errors

**Решение:**

1. Увеличьте timeout:

```bash
export OPENCODE_TIMEOUT=600
```

2. Проверьте ресурсы:

```bash
# Проверьте CPU
top

# Проверьте память
free -h
```

3. Уменьшите сложность задач:

```rust
// Вместо одной большой задачи
// Разбейте на несколько маленьких
```

---

## Opencode проблемы

### Проблема: Opencode сервер недоступен

**Симптомы:**

```
Error: Failed to connect to Opencode server: Connection refused
```

**Решение:**

1. Запустите Opencode сервер:

```bash
opencode serve --hostname=127.0.0.1 --port=4096
```

2. Проверьте, что порт не занят:

```bash
lsof -i :4096
```

3. Проверьте firewall:

```bash
# UFW
sudo ufw allow 4096

# iptables
sudo iptables -A INPUT -p tcp --dport 4096 -j ACCEPT
```

---

### Проблема: Architect agent не найден

**Симптомы:**

```
Error: Agent not found: architect
```

**Решение:**

1. Проверьте, что architect agent создан:

```bash
ls -la .opencode/agent/
```

2. Создайте architect agent:

```bash
# См. configuration/setup.sh
./configuration/setup.sh
```

3. Или создайте вручную:

```bash
cat > .opencode/agent/architect.md << 'EOF'
---
description: Orchestrates complex multi-step development tasks
mode: primary
permission:
  task:
    "*": "allow"
  edit: "allow"
  bash: "allow"
  read: "allow"
  glob: "allow"
  grep: "allow"
---

You are an architect agent...
EOF
```

---

### Проблема: Git операции не работают

**Симптомы:**

```
Error: Git not configured
```

**Решение:**

1. Настройте git:

```bash
git config --global user.name "Your Name"
git config --global user.email "your@email.com"
```

2. Проверьте конфигурацию:

```bash
git config --global --list
```

3. Проверьте, что репозиторий инициализирован:

```bash
cd /path/to/project
git status
```

---

### Проблема: Session не создается

**Симптомы:**

```
Error: Failed to create session: 400 Bad Request
```

**Решение:**

1. Проверьте формат запроса:

```rust
// Правильно
let body = serde_json::json!({
  "title": "Task",
  "agent": "architect"
});

// Неправильно
let body = serde_json::json!({
  "name": "Task",  // Должно быть "title"
  "type": "architect"  // Должно быть "agent"
});
```

2. Проверьте Opencode API документацию:

```bash
open http://127.0.0.1:4096/doc
```

---

## Sandbox проблемы

### Проблема: Команда не найдена

**Симптомы:**

```
Error: Command failed (exit code 127): sh: yt-dlp: command not found
```

**Решение:**

1. Проверьте Docker образ:

```bash
docker images | grep agent-sandbox
```

2. Пересоберите образ с нужными инструментами:

```dockerfile
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y \
    python3 pip \
    ffmpeg \
    yt-dlp
```

3. Проверьте, что инструмент установлен в контейнере:

```bash
docker exec agent-sandbox-user-123 which yt-dlp
```

---

### Проблема: Контейнер не запускается

**Симптомы:**

```
Error: Failed to start container
```

**Решение:**

1. Проверьте Docker демона:

```bash
sudo systemctl status docker
```

2. Проверьте логи контейнера:

```bash
docker logs agent-sandbox-user-123
```

3. Проверьте, что образ существует:

```bash
docker images | grep agent-sandbox
```

4. Пересоздайте контейнер:

```bash
docker rm -f agent-sandbox-user-123
docker run -d --name agent-sandbox-user-123 agent-sandbox:latest
```

---

### Проблема: Нет доступа к файлам

**Симптомы:**

```
Error: File not found: /workspace/file.txt
```

**Решение:**

1. Проверьте рабочую директорию:

```bash
export SANBOX_WORKSPACE=/workspace
```

2. Проверьте права доступа:

```bash
docker exec agent-sandbox-user-123 ls -la /workspace
```

3. Проверьте, что volume монтируется:

```bash
docker inspect agent-sandbox-user-123 | grep -A 10 Mounts
```

---

## LLM проблемы

### Проблема: LLM не выбирает правильный инструмент

**Симптомы:**

- LLM использует sandbox для кода
- LLM использует opencode для данных

**Решение:**

1. Улучшите system prompt (см. [configuration/llm_prompt.md](../configuration/llm_prompt.md))
2. Добавьте примеры в prompt:

```markdown
When to use Sandbox:

- Download files (yt-dlp, curl)
- Process media (ffmpeg)
- Run Python scripts

When to use Opencode:

- Implement code features
- Fix bugs
- Refactor code
- Git operations
```

3. Используйте более детерминированную temperature:

```bash
export LLM_TEMPERATURE=0.2
```

---

### Проблема: LLM не понимает задачу

**Симптомы:**

- LLM делает неправильные вещи
- LLM не завершает задачу

**Решение:**

1. Будьте более конкретны в запросе:

```
Плохо: "исправь логин"
Хорошо: "исправь баг с 500 ошибкой при логине пользователя в /api/auth/login"
```

2. Предоставьте контекст:

```
"В проекте используется Express.js и TypeScript. Исправь баг..."
```

3. Разбейте сложные задачи:

```
"Сначала найди место с багом, потом исправь его, потом протестируй"
```

---

## Сетевые проблемы

### Проблема: Timeout ошибки

**Симптомы:**

```
Error: Request timeout after 300 seconds
```

**Решение:**

1. Увеличьте timeout:

```bash
export OPENCODE_TIMEOUT=600
```

2. Или в коде:

```rust
let provider = OpencodeToolProvider::new(url)
    .with_timeout(Duration::from_secs(600));
```

3. Проверьте сетевое соединение:

```bash
ping 127.0.0.1
```

---

### Проблема: SSL ошибки

**Симптомы:**

```
Error: SSL handshake failed
```

**Решение:**

1. Для self-signed сертификатов (только для разработки!):

```bash
export OPENCODE_BASE_URL=http://127.0.0.1:4096  # Не HTTPS
```

2. Или отключите проверку сертификата (только для разработки!):

```rust
let client = reqwest::Client::builder()
    .danger_accept_invalid_certs(true)  // НЕ ДЛЯ ПРОДАКШЕНА!
    .build()?;
```

---

## Отладка

### Включить подробное логирование

```bash
export RUST_LOG=trace
export LOG_FILE=./logs/debug.log
```

### Проверить HTTP запросы

```rust
// В OpencodeToolProvider
let client = reqwest::Client::builder()
    .timeout(Duration::from_secs(300))
    .add_default_header("X-Debug", "true")
    .build()?;
```

### Мониторинг Docker контейнера

```bash
# Логи контейнера
docker logs -f agent-sandbox-user-123

# Статистика
docker stats agent-sandbox-user-123

# Процессы
docker exec agent-sandbox-user-123 ps aux
```

---

## Получение помощи

### Логи

1. Проверьте логи приложения:

```bash
cat logs/agent.log
```

2. Проверьте логи Opencode:

```bash
# Opencode логи (если запущен через systemd)
journalctl -u opencode -f
```

3. Проверьте Docker логи:

```bash
docker logs agent-sandbox-user-123
```

### Сообщество

1. GitHub Issues: https://github.com/your-repo/issues
2. Discord: https://discord.gg/your-server
3. Stack Overflow: тег `opencode-sandbox`

---

## Следующие шаги

- [ ] Изучить [unit_tests.md](./unit_tests.md) - Unit тесты
- [ ] Изучить [integration_tests.md](./integration_tests.md) - Интеграционные тесты
- [ ] Перейти к [deployment/](../deployment/) - Деплой

---

**Связанные документы:**

- [testing/unit_tests.md](./unit_tests.md) - Unit тесты
- [testing/integration_tests.md](./integration_tests.md) - Интеграционные тесты
- [configuration/setup.sh](../configuration/setup.sh) - Скрипт настройки
