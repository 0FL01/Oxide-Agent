# Monitoring

> **Мониторинг и логирование**
>
> 📁 **Раздел:** Deployment
> 🎯 **Цель:** Отслеживание состояния системы

---

## 📋 Оглавление

- [Логирование](#логирование)
- [Метрики](#метрики)
- [Health Checks](#health-checks)
- [Alerts](#alerts)
- [Дашборды](#дашборды)
- [Troubleshooting](#troubleshooting)

---

## Логирование

### Уровни логирования

| Уровень   | Описание                 | Когда использовать |
| --------- | ------------------------ | ------------------ |
| **ERROR** | Критические ошибки       | Всегда             |
| **WARN**  | Предупреждения           | Всегда             |
| **INFO**  | Общая информация         | Prod               |
| **DEBUG** | Отладка                  | Dev/Testing        |
| **TRACE** | Максимальная детализация | Dev/Testing        |

### Конфигурация

```bash
# Prod
export RUST_LOG=info

# Dev
export RUST_LOG=debug

# Testing
export RUST_LOG=trace
```

### Структура логов

```
[2024-03-05T10:30:45.123Z INFO opencode_provider] Creating session: task="list files"
[2024-03-05T10:30:45.456Z INFO opencode_provider] Session created: id="session-abc123"
[2024-03-05T10:30:46.789Z ERROR opencode_provider] Failed to send prompt: Connection refused
[2024-03-05T10:30:47.012Z WARN tool_registry] Opencode health check failed
```

### Локация логов

```bash
# Application logs
/var/log/agent/app.log

# Opencode logs
/var/log/agent/opencode.log

# Sandbox logs
/var/log/agent/sandbox.log

# All logs (combined)
/var/log/agent/combined.log
```

### Log rotation

```bash
# /etc/logrotate.d/agent
/var/log/agent/*.log {
    daily
    rotate 14
    compress
    delaycompress
    notifempty
    create 0644 agent agent
    sharedscripts
    postrotate
        systemctl reload agent > /dev/null 2>&1 || true
    endscript
}
```

---

## Метрики

### Ключевые метрики

#### Opencode

| Метрика                        | Тип       | Описание                     | Alert    |
| ------------------------------ | --------- | ---------------------------- | -------- |
| **opencode_requests_total**    | Counter   | Общее количество запросов    | -        |
| **opencode_requests_duration** | Histogram | Время выполнения запросов    | > 30s    |
| **opencode_errors_total**      | Counter   | Общее количество ошибок      | > 10/min |
| **opencode_sessions_active**   | Gauge     | Количество активных sessions | > 100    |
| **opencode_health**            | Gauge     | Health status (0/1)          | = 0      |

#### Sandbox

| Метрика                       | Тип       | Описание                        | Alert   |
| ----------------------------- | --------- | ------------------------------- | ------- |
| **sandbox_commands_total**    | Counter   | Общее количество команд         | -       |
| **sandbox_commands_duration** | Histogram | Время выполнения команд         | > 60s   |
| **sandbox_errors_total**      | Counter   | Общее количество ошибок         | > 5/min |
| **sandbox_containers_active** | Gauge     | Количество активных контейнеров | > 50    |
| **sandbox_memory_usage**      | Gauge     | Использование памяти            | > 90%   |
| **sandbox_cpu_usage**         | Gauge     | Использование CPU               | > 80%   |

#### LLM

| Метрика                   | Тип       | Описание                  | Alert    |
| ------------------------- | --------- | ------------------------- | -------- |
| **llm_requests_total**    | Counter   | Общее количество запросов | -        |
| **llm_requests_duration** | Histogram | Время выполнения запросов | > 60s    |
| **llm_tokens_total**      | Counter   | Общее количество токенов  | -        |
| **llm_cost_total**        | Counter   | Общая стоимость ($)       | > budget |

### Prometheus exporter

```rust
use prometheus::{Counter, Histogram, Registry, register_counter, register_histogram};

lazy_static! {
    static ref REGISTRY: Registry = Registry::new();

    static ref OPENCODE_REQUESTS_TOTAL: Counter = register_counter!(
        "opencode_requests_total",
        "Total number of Opencode requests"
    ).unwrap();

    static ref OPENCODE_REQUESTS_DURATION: Histogram = register_histogram!(
        "opencode_requests_duration_seconds",
        "Duration of Opencode requests in seconds"
    ).unwrap();
}

pub fn metrics() -> String {
    REGISTRY.gather()
}
```

### Endpoint

```rust
// HTTP endpoint для метрик
use hyper::{Body, Request, Response, Server};

async fn metrics_handler(_req: Request<Body>) -> Result<Response<Body>, Error> {
    let metrics = metrics();
    Ok(Response::builder()
        .status(200)
        .header("Content-Type", "text/plain")
        .body(Body::from(metrics))
        .unwrap())
}
```

---

## Health Checks

### Opencode Health

```bash
#!/bin/bash
# health_check.sh

# Проверить Opencode сервер
curl -f http://opencode:4096/vcs || {
    echo "Opencode health check failed"
    exit 1
}

# Проверить git
cd /path/to/project
git status --short || {
    echo "Git health check failed"
    exit 1
}

# Проверить архитектор agent
test -f .opencode/agent/architect.md || {
    echo "Architect agent not found"
    exit 1
}

echo "All health checks passed"
```

### Docker Health

```yaml
# docker-compose.yml
healthcheck:
  test: ["CMD", "curl", "-f", "http://localhost:4096/vcs"]
  interval: 30s
  timeout: 10s
  retries: 3
  start_period: 40s
```

### API Endpoint

```rust
use hyper::{Body, Request, Response, Server, StatusCode};

async fn health_handler(_req: Request<Body>) -> Result<Response<Body>, Error> {
    // Проверить Opencode
    let opencode_healthy = check_opencode_health().await;

    // Проверить Sandbox
    let sandbox_healthy = check_sandbox_health().await;

    let status = if opencode_healthy && sandbox_healthy {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    let body = serde_json::json!({
        "status": if status == StatusCode::OK { "healthy" } else { "unhealthy" },
        "checks": {
            "opencode": opencode_healthy,
            "sandbox": sandbox_healthy,
        }
    });

    Ok(Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap())
}
```

---

## Alerts

### Alerting правила

#### Critical alerts

```yaml
# alerts.yml
groups:
  - name: critical
    interval: 30s
    rules:
      # Opencode health check failed
      - alert: OpencodeHealthCheckFailed
        expr: opencode_health == 0
        for: 1m
        labels:
          severity: critical
        annotations:
          summary: "Opencode health check failed"
          description: "Opencode server is not responding"

      # High error rate
      - alert: HighErrorRate
        expr: rate(opencode_errors_total[5m]) > 10
        for: 2m
        labels:
          severity: critical
        annotations:
          summary: "High error rate detected"
          description: "Error rate is > 10 errors/minute for 2 minutes"

      # High memory usage
      - alert: HighMemoryUsage
        expr: sandbox_memory_usage > 0.9
        for: 5m
        labels:
          severity: critical
        annotations:
          summary: "High memory usage"
          description: "Memory usage > 90% for 5 minutes"
```

#### Warning alerts

```yaml
- name: warnings
  interval: 1m
  rules:
    # Slow requests
    - alert: SlowRequests
      expr: histogram_quantile(0.95, opencode_requests_duration) > 30
      for: 5m
      labels:
        severity: warning
      annotations:
        summary: "Slow Opencode requests"
        description: "95th percentile of request duration > 30s"

    # High CPU usage
    - alert: HighCPUUsage
      expr: sandbox_cpu_usage > 0.8
      for: 10m
      labels:
        severity: warning
      annotations:
        summary: "High CPU usage"
        description: "CPU usage > 80% for 10 minutes"
```

### Notification каналы

```yaml
# alertmanager.yml
receivers:
  - name: slack
    slack_configs:
      - api_url: "https://hooks.slack.com/services/..."
        channel: "#alerts"

  - name: pagerduty
    pagerduty_configs:
      - service_key: "***"

  - name: email
    email_configs:
      - to: "oncall@example.com"

route:
  receiver: "slack"
  routes:
    - match:
        severity: critical
      receiver: "pagerduty"
```

---

## Дашборды

### Grafana дашборды

#### Overview Dashboard

**Panels:**

1. **Request Rate** - requests/min (Opencode + Sandbox)
2. **Error Rate** - errors/min (Opencode + Sandbox)
3. **Latency** - P50, P95, P99 (Opencode)
4. **Active Sessions** - Current active sessions
5. **Active Containers** - Current active containers
6. **Resource Usage** - CPU, Memory (Sandbox)

#### Opencode Dashboard

**Panels:**

1. **Requests Total** - Counter
2. **Requests Duration** - Histogram
3. **Errors Total** - Counter
4. **Sessions Active** - Gauge
5. **Health Status** - Gauge
6. **Recent Errors** - Logs

#### Sandbox Dashboard

**Panels:**

1. **Commands Total** - Counter
2. **Commands Duration** - Histogram
3. **Errors Total** - Counter
4. **Containers Active** - Gauge
5. **Memory Usage** - Gauge
6. **CPU Usage** - Gauge
7. **Recent Errors** - Logs

---

## Troubleshooting

### Поиск проблем в логах

```bash
# Найти ошибки
grep ERROR /var/log/agent/*.log

# Найти последние 10 ошибок
grep ERROR /var/log/agent/*.log | tail -10

# Найти ошибки за последний час
grep "ERROR" /var/log/agent/*.log | grep "$(date +'%Y-%m-%d')"

# Подсчитать количество ошибок
grep ERROR /var/log/agent/*.log | wc -l
```

### Анализ метрик

```bash
# Получить метрики
curl http://localhost:9090/metrics

# Запрос Prometheus для rate ошибок
curl 'http://localhost:9090/api/v1/query?query=rate(opencode_errors_total[5m])'

# Запрос для 95th percentile latency
curl 'http://localhost:9090/api/v1/query?query=histogram_quantile(0.95, opencode_requests_duration)'
```

### Live monitoring

```bash
# Следить за логами в реальном времени
tail -f /var/log/agent/combined.log

# Следить за метриками
watch -n 1 'curl -s http://localhost:9090/metrics | grep opencode'

# Проверить статус контейнеров
watch -n 1 'docker stats --no-stream'
```

---

## Следующие шаги

- [ ] Настроить Prometheus
- [ ] Настроить Grafana дашборды
- [ ] Настроить Alertmanager
- [ ] Тестировать alerts
- [ ] Документировать процедуры

---

**Связанные документы:**

- [deployment/production_checklist.md](./production_checklist.md) - Чек-лист для продакшена
- [testing/troubleshooting.md](../testing/troubleshooting.md) - Устранение проблем
- [architecture/components.md](../architecture/components.md) - Компоненты системы
