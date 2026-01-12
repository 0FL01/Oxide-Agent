# Sandbox Disk Quota Implementation: Loop Device + XFS Project Quotas

## Обзор

Документация реализации ограничения дискового пространства для Docker-контейнеров sandbox среды агента на хост-системе Debian 13 (kernel 6.12, ext4).

**Архитектура:**
- Хост: ext4 filesystem
- Loop device + sparse file → XFS filesystem с prjquota
- Bind mount из XFS-директории с project quota → Docker container /workspace

```
┌─────────────────────────────────────────────────────────────────┐
│                         Host (Debian 13, ext4)                  │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │  /var/lib/sandbox-storage.img (sparse file, e.g. 10GB)    │  │
│  │  └── XFS filesystem, mounted with prjquota                │  │
│  │      └── /mnt/sandbox-storage/                            │  │
│  │          ├── user_123456/ (project ID 1, quota 500MB)     │  │
│  │          ├── user_789012/ (project ID 2, quota 500MB)     │  │
│  │          └── ...                                          │  │
│  └───────────────────────────────────────────────────────────┘  │
│                              ▼ bind mount                       │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │  Docker Container (agent-sandbox-123456)                  │  │
│  │  /workspace ← bind mount from /mnt/sandbox-storage/user_X │  │
│  └───────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

## Зачем нужно решение

- **Overlay2 не поддерживает storage-opt size на ext4**
- Хост ext4, разметка в xfs невозможна
- tmpfs не подходит (перезагрузка потеряет данные)
- Loop device с XFS + project quotas - рекомендуемый путь для per-user квот

---

## Шаг 1: Подготовка хоста

### 1.1 Установка пакетов

```bash
apt update
apt install -y xfsprogs quota
```

### 1.2 Создание sparse-файла

Sparse-файл занимает место на диске только при фактическом использовании.

```bash
mkdir -p /var/lib/sandbox-storage

# Создаём sparse-файл (10GB виртуально, физически ~0 байт)
truncate -s 10G /var/lib/sandbox-storage.img

# Проверка
ls -lsh /var/lib/sandbox-storage.img
# Ожидаемый вывод: 0 -rw-r--r-- 1 root root 10G ...
```

### 1.3 Форматирование в XFS

```bash
mkfs.xfs -f /var/lib/sandbox-storage.img
```

### 1.4 Монтирование с prjquota

```bash
mkdir -p /mnt/sandbox-storage

# Монтируем с prjquota
mount -o loop,prjquota /var/lib/sandbox-storage.img /mnt/sandbox-storage

# Проверяем
mount | grep sandbox-storage
# Ожидаемый вывод: /var/lib/sandbox-storage.img on /mnt/sandbox-storage type xfs (...,prjquota)

# Проверяем состояние квот
xfs_quota -x -c 'state' /mnt/sandbox-storage
# Должно показать: Project quota state on /mnt/sandbox-storage (/dev/loopX)
#                  Enforcement: ON
```

---

## Шаг 2: Автоматическое монтирование

### Вариант A: systemd mount unit (рекомендуемый для Debian 13)

Файл: `/etc/systemd/system/mnt-sandbox\x2dstorage.mount`

```ini
[Unit]
Description=Sandbox XFS Storage with Project Quotas
After=local-fs.target
Before=docker.service

[Mount]
What=/var/lib/sandbox-storage.img
Where=/mnt/sandbox-storage
Type=xfs
Options=loop,prjquota

[Install]
WantedBy=multi-user.target
```

Активация:
```bash
systemctl daemon-reload
systemctl enable mnt-sandbox\\x2dstorage.mount
systemctl start mnt-sandbox\\x2dstorage.mount
```

### Вариант B: /etc/fstab

```bash
echo '/var/lib/sandbox-storage.img /mnt/sandbox-storage xfs loop,prjquota 0 0' >> /etc/fstab
mount /mnt/sandbox-storage
```

---

## Шаг 3: Управление Project Quotas

### 3.1 Концепция

XFS project quotas позволяют назначать **project ID** директории и ограничивать использование диска.

- Project ID = любой номер (1-2^64)
- Используем Telegram user ID как project ID
- Лимит задается в байтах/блоках

### 3.2 Создание квоты для пользователя

Скрипт: `/usr/local/bin/setup_user_quota.sh`

```bash
#!/bin/bash

USER_ID=$1
QUOTA_MB=${2:-500}  # По умолчанию 500MB
PROJECT_ID=$USER_ID
STORAGE_PATH="/mnt/sandbox-storage"
USER_DIR="$STORAGE_PATH/user_$USER_ID"

# Создаём директорию
mkdir -p "$USER_DIR"
chmod 755 "$USER_DIR"

# Назначаем project ID директории
xfs_quota -x -c "project -s -p $USER_DIR $PROJECT_ID" "$STORAGE_PATH"

# Устанавливаем лимит (bhard = жёсткий лимит в MB)
xfs_quota -x -c "limit -p bhard=${QUOTA_MB}m $PROJECT_ID" "$STORAGE_PATH"

echo "Created quota for user $USER_ID: ${QUOTA_MB}MB at $USER_DIR"
```

Использование:
```bash
chmod +x /usr/local/bin/setup_user_quota.sh
/usr/local/bin/setup_user_quota.sh 123456 500
/usr/local/bin/setup_user_quota.sh 789012 1000
```

### 3.3 Проверка квот

```bash
# Показать все project quotas
xfs_quota -x -c 'report -p -h' /mnt/sandbox-storage

# Пример вывода:
# Project ID   Used   Soft   Hard   Warn/Grace
# ---------- ------- ------ ------ -----------
# 123456       12M      0    500M  00 [------]
# 789012      256M      0    500M  00 [------]

# Проверить конкретный проект
xfs_quota -x -c 'quota -p -h 123456' /mnt/sandbox-storage
```

### 3.4 Сброс/удаление квоты

Квота автоматически удаляется при удалении директории:

```bash
rm -rf /mnt/sandbox-storage/user_123456
```

Нет необходимости явно удалять квоту - XFS очистит неиспользуемые project ID.

---

## Шаг 4: Изменения в коде (Rust)

### 4.1 Новые константы в `src/config.rs`

```rust
/// Path to XFS storage mount point
pub const SANDBOX_STORAGE_PATH: &str = "/mnt/sandbox-storage";

/// Disk quota per sandbox in megabytes
pub const SANDBOX_DISK_QUOTA_MB: u64 = 500;
```

### 4.2 Модификация `create_sandbox()` в `src/sandbox/manager.rs`

```rust
use anyhow::{anyhow, Context, Result};
// ... existing imports ...

impl SandboxManager {
    /// Create and start a new sandbox container
    pub async fn create_sandbox(&mut self) -> Result<()> {
        if self.container_id.is_some() {
            return Ok(());
        }

        let container_name = format!("agent-sandbox-{}", self.user_id);

        // НОВОЕ: Инициализируем storage с квотой
        self.ensure_user_storage().await?;

        // Проверка существующего контейнера...
        let mut filters = HashMap::new();
        filters.insert("name".to_string(), vec![container_name.clone()]);

        let containers = self
            .docker
            .list_containers(Some(bollard::query_parameters::ListContainersOptions {
                all: true,
                filters: Some(filters),
                ..Default::default()
            }))
            .await
            .context("Failed to list containers")?;

        if let Some(container) = containers.first() {
            let id = container.id.clone().unwrap_or_default();
            self.container_id = Some(id.clone());
            // ... existing logic for starting container
            return Ok(());
        }

        // НОВОЕ: Bind mount с квотированной директорией
        let user_workspace = format!("{}/user_{}", SANDBOX_STORAGE_PATH, self.user_id);

        let host_config = HostConfig {
            memory: Some(SANDBOX_MEMORY_LIMIT),
            cpu_period: Some(SANDBOX_CPU_PERIOD),
            cpu_quota: Some(SANDBOX_CPU_QUOTA),
            network_mode: Some("bridge".to_string()),
            auto_remove: Some(true),
            binds: Some(vec![
                format!("{}:/workspace:rw", user_workspace),
            ]),
            ..Default::default()
        };

        let config = ContainerCreateBody {
            image: Some(self.image_name.clone()),
            hostname: Some("sandbox".to_string()),
            working_dir: Some("/workspace".to_string()),
            host_config: Some(host_config),
            labels: Some(HashMap::from([
                ("agent.user_id".to_string(), self.user_id.to_string()),
                ("agent.sandbox".to_string(), "true".to_string()),
            ])),
            cmd: Some(vec!["sleep".to_string(), "infinity".to_string()]),
            ..Default::default()
        };

        // ... rest of container creation logic
    }

    /// Ensure user storage directory exists with XFS project quota applied
    async fn ensure_user_storage(&self) -> Result<()> {
        use tokio::process::Command;
        use tracing::{info, warn};

        let user_dir = format!("{}/user_{}", SANDBOX_STORAGE_PATH, self.user_id);
        let project_id = self.user_id.to_string();

        // Создаём директорию
        tokio::fs::create_dir_all(&user_dir).await
            .with_context(|| format!("Failed to create user storage directory: {}", user_dir))?;

        // Назначаем project ID (idempotent - можно запускать многократно)
        let setup_project = Command::new("xfs_quota")
            .args(["-x", "-c", &format!("project -s -p {} {}", user_dir, project_id)])
            .arg(SANDBOX_STORAGE_PATH)
            .output()
            .await
            .context("Failed to setup XFS project")?;

        if !setup_project.status.success() {
            warn!(
                user_id = self.user_id,
                stderr = %String::from_utf8_lossy(&setup_project.stderr),
                "xfs_quota project setup warning (may be already configured)"
            );
        }

        // Устанавливаем лимит (bhard = hard limit in blocks, суффикс 'm' для MB)
        let set_limit = Command::new("xfs_quota")
            .args(["-x", "-c", &format!(
                "limit -p bhard={}m {}",
                SANDBOX_DISK_QUOTA_MB,
                project_id
            )])
            .arg(SANDBOX_STORAGE_PATH)
            .output()
            .await
            .context("Failed to set XFS quota limit")?;

        if !set_limit.status.success() {
            anyhow::bail!(
                "Failed to set disk quota for user {}: {}",
                self.user_id,
                String::from_utf8_lossy(&set_limit.stderr)
            );
        }

        info!(
            user_id = self.user_id,
            quota_mb = SANDBOX_DISK_QUOTA_MB,
            path = %user_dir,
            "User storage initialized with XFS quota"
        );

        Ok(())
    }
}
```

### 4.3 Новый метод: проверка использования диска

```rust
impl SandboxManager {
    /// Get current disk usage and quota limit for the user
    pub async fn get_disk_quota_info(&self) -> Result<(u64, u64)> {
        use tokio::process::Command;

        let project_id = self.user_id.to_string();

        let output = Command::new("xfs_quota")
            .args(["-x", "-c", &format!("quota -p -b {}", project_id)])
            .arg(SANDBOX_STORAGE_PATH)
            .output()
            .await
            .context("Failed to get quota info")?;

        // Парсинг вывода (формат зависит от версии xfs_quota)
        // Пример:
        //   Disk quotas for Project #123456 (123456)
        //   Filesystem  blocks  quota  limit  grace  files  quota  limit  grace
        //   /mnt/...    12345   0      512000 ...

        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().collect();

        if lines.len() < 3 {
            anyhow::bail!("Unexpected xfs_quota output format");
        }

        // Парсим блоки (вторую строку)
        let data_line = lines.get(2).ok_or_else(|| anyhow!("Missing data line"))?;
        let parts: Vec<&str> = data_line.split_whitespace().collect();

        // Порядок: Filesystem, blocks, quota, limit, grace, files, quota, limit, grace
        let blocks: u64 = parts
            .get(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let limit: u64 = parts
            .get(3)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        // Блоки XFS = 512 байт
        let used_bytes = blocks * 512;
        let limit_bytes = limit * 512;

        Ok((used_bytes, limit_bytes))
    }

    /// Check if disk quota is exceeded (returns true if exceeded)
    pub async fn is_quota_exceeded(&self) -> Result<bool> {
        let (used, limit) = self.get_disk_quota_info().await?;
        Ok(used >= limit)
    }
}
```

### 4.4 Интеграция в exec_command

Добавить проверку квоты перед выполнением команд, которые пишут данные:

```rust
pub async fn write_file(&self, path: &str, content: &[u8]) -> Result<()> {
    if self.container_id.is_none() {
        return Err(anyhow!("Sandbox not running"));
    }

    // НОВОЕ: Проверяем квоту перед записью
    let content_size = content.len() as u64;
    let (current_used, limit) = self.get_disk_quota_info().await?;

    if current_used + content_size > limit {
        anyhow::bail!(
            "Disk quota exceeded: {}MB / {}MB (attempting to add {} bytes)",
            current_used / 1024 / 1024,
            limit / 1024 / 1024,
            content_size
        );
    }

    // ... существующая логика записи
}
```

---

## Шаг 5: Docker Daemon Configuration

Файл: `/etc/docker/daemon.json`

```json
{
  "log-driver": "json-file",
  "log-opts": {
    "max-size": "8m",
    "max-file": "3",
    "compress": "true"
  },
  "iptables": false,
  "dns": ["1.1.1.1", "9.9.9.9"],
  "data-root": "/var/lib/docker"
}
```

**Примечание:** Никаких изменений storage driver не требуется. overlay2 остаётся, квота работает через bind mount из XFS.

Перезапуск:
```bash
systemctl restart docker
```

---

## Шаг 6: Systemd oneshot service для инициализации

Файл: `/etc/systemd/system/sandbox-storage-init.service`

```ini
[Unit]
Description=Initialize Sandbox Storage Quotas
After=mnt-sandbox\x2dstorage.mount
Before=docker.service
Requires=mnt-sandbox\x2dstorage.mount

[Service]
Type=oneshot
ExecStart=/usr/local/bin/sandbox-storage-init.sh
RemainAfterExit=yes

[Install]
WantedBy=multi-user.target
```

Скрипт инициализации: `/usr/local/bin/sandbox-storage-init.sh`

```bash
#!/bin/bash
set -e

# Убедимся, что storage смонтирован
if ! mountpoint -q /mnt/sandbox-storage; then
    echo "ERROR: Sandbox storage not mounted"
    exit 1
fi

# Создаём базовую структуру (опционально)
mkdir -p /mnt/sandbox-storage/users

echo "Sandbox storage initialized successfully"
```

```bash
chmod +x /usr/local/bin/sandbox-storage-init.sh
systemctl daemon-reload
systemctl enable sandbox-storage-init.service
```

---

## Важные замечания

### 1. Права доступа

Контейнер работает от root внутри, bind mount наследует права хоста.

```bash
chmod 755 /mnt/sandbox-storage
# Для каждой user директории (позволяет контейнеру писать)
chmod 777 /mnt/sandbox-storage/user_*
```

### 2. Расширение storage при необходимости

```bash
# 1. Размонтировать
umount /mnt/sandbox-storage

# 2. Увеличить sparse файл
truncate -s 20G /var/lib/sandbox-storage.img

# 3. Расширить XFS (online - без размонтирования!)
# Но так как loop device, нужно перемонтировать
mount -o loop,prjquota /var/lib/sandbox-storage.img /mnt/sandbox-storage
xfs_growfs /mnt/sandbox-storage
```

### 3. Мониторинг

```bash
# Общее использование
df -h /mnt/sandbox-storage

# Per-project usage
xfs_quota -x -c 'report -p -h' /mnt/sandbox-storage

# Количество пользователей
ls -1 /mnt/sandbox-storage | grep "^user_" | wc -l
```

### 4. Очистка при удалении пользователя

```bash
# Удалить директорию
rm -rf /mnt/sandbox-storage/user_$USER_ID

# Квота автоматически освободится
# Можно запустить periodic cleanup через systemd timer
```

### 5. Backup

Backup sparse-файла:

```bash
# Sparse-aware copy
cp --sparse=always /var/lib/sandbox-storage.img /backup/sandbox-storage.img.backup
```

---

## Ссылки

- [Mounting loopback ext4/xfs filesystem to enforce limits](https://fabianlee.org/2020/01/13/linux-mounting-a-loopback-ext4-xfs-filesystem-to-isolate-or-enforce-storage-limits/)
- [Using XFS project quotas to limit capacity](https://fabianlee.org/2020/01/13/linux-using-xfs-project-quotas-to-limit-capacity-within-a-subdirectory/)
- [OpenEBS XFS Quota with Loop Device](https://openebs.io/docs/user-guides/local-storage-user-guide/local-pv-hostpath/advanced-operations/xfs-quota/loop-device-xfs-quota)
- [Docker Storage Quota per container](https://forums.docker.com/t/storage-quota-per-container-overlay2-backed-by-xfs/37653)
- [Docker Container Size Quota](https://reece.tech/posts/docker-container-size-quota/)

---

## Вопросы для уточнения перед реализацией

1. **Размер общего storage**: 10GB достаточно для начала? Сколько активных пользователей ожидается?

2. **Квота на пользователя**: 500MB по умолчанию - подходит? Нужна ли динамическая настройка (меньше для новых, больше для premium)?

3. **Persistence**: Данные пользователей должны сохраняться между пересозданиями контейнера? (Да - через bind mount)

4. **Cleanup policy**: Нужен ли автоматический cleanup неактивных пользователей (например, удаление данных старше 30 дней)?

5. **Мониторинг**: Добавить ли в bot команду `/quota` для показа пользователю его использования?

---

## План тестирования

1. Создать sandbox storage и назначить квоту
2. Запустить контейнер с bind mount
3. Записать файлы внутри контейнера через `write_file()`
4. Проверить `get_disk_quota_info()` возвращает корректные данные
5. Попытаться превысить квоту - убедиться, что ошибка возвращается
6. Удалить контейнер и создать заново - данные должны остаться
7. Удалить user директорию - квота должна очиститься

---

*Последнее обновление: 2025-01-09*
