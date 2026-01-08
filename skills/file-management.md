---
name: file-management
description: Работа с песочницей, файлами и выполнением команд.
triggers: [файл, папк, директори, команда, скрипт, выполнить, python, bash, sandbox, ls, cat, grep, rm, cp, mv]
allowed_tools: [execute_command, write_file, read_file, send_file_to_user, list_files]
weight: medium
---
## Sandbox (выполнение кода):
- **execute_command**: выполнить bash-команду в sandbox (доступны: python3, pip, ffmpeg, yt-dlp, curl, wget, date, cat, ls, grep и другие стандартные утилиты, если утилиты нет — то поставь её)
- **write_file**: записать содержимое в файл
- **read_file**: прочитать содержимое файла
- **send_file_to_user**: отправить файл из песочницы пользователю в Telegram
  - Поддерживает как абсолютные (/workspace/file.txt), так и относительные (file.txt) пути
  - Автоматически ищет файл в /workspace, если указано только имя
  - Если найдено несколько файлов с одинаковым именем — попросит уточнить путь
  - ⚠️ Лимит Telegram: если файл > 50 МБ, используй `upload_file`
- **list_files**: показать содержимое директории в песочнице (по умолчанию /workspace)

## Важные правила:
- **СЕТЬ**: У тебя ЕСТЬ доступ к интернету (curl, wget, pip, git работают). Ошибки "command not found" означают отсутствие утилиты, а не сети.
- **УСТАНОВКА**: Если утилиты нет (dig, ping и т.д.) — выполни `apt-get update && apt-get install -y <package>`, затем используй.
- Если нужна текущая дата — вызови execute_command с командой `date`
- Для вычислений используй Python: execute_command с `python3 -c "..."`
