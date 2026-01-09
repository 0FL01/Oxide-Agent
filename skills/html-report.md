---
name: html-report
description: Навык для генерации HTML-отчетов, дашбордов и страниц в стиле Playful Material Design 3.
triggers: [html, отчёт, report, web, design, дизайн, страница, css, верстка]
weight: medium
---

При генерации HTML-страниц, отчетов или веб-интерфейсов, следуй стилю **Playful Material Design 3**.

### Принципы дизайна
1.  **Playful & Dynamic**: Используй насыщенные цвета, игривые формы и анимации.
2.  **Pill-Shaped Buttons**: Все кнопки должны иметь полную скругленность (border-radius: 9999px).
3.  **Distinct Elevation**: Используй тени для создания глубины. Элементы должны "парить" над поверхностью.
4.  **Micro-interactions**: Добавляй эффекты при наведении (scale, shadow lift), нажатии (ripple).

### Базовый CSS шаблон (обязательно включи или адаптируй это в `<style>`):

```css
:root {
  /* Playful Color Scheme based on easy HSL manipulation */
  --hue: 265; /* Violet/Purple base */
  --md-sys-color-primary: hsl(var(--hue), 85%, 60%);
  --md-sys-color-on-primary: hsl(0, 0%, 100%);
  --md-sys-color-primary-container: hsl(var(--hue), 90%, 96%);
  --md-sys-color-on-primary-container: hsl(var(--hue), 40%, 20%);

  --md-sys-color-secondary: hsl(calc(var(--hue) + 40), 70%, 55%);
  --md-sys-color-on-secondary: hsl(0, 0%, 100%);
  
  --md-sys-color-surface: hsl(240, 10%, 98%);
  --md-sys-color-surface-container: hsl(0, 0%, 100%);
  --md-sys-color-on-surface: hsl(240, 10%, 10%);
  
  --md-sys-elevation-1: 0 2px 6px rgba(0,0,0,0.05), 0 1px 3px rgba(0,0,0,0.1);
  --md-sys-elevation-2: 0 6px 16px rgba(0,0,0,0.08), 0 3px 6px rgba(0,0,0,0.12);
  --md-sys-elevation-hover: 0 10px 24px rgba(var(--hue),100%,70%,0.25), 0 4px 8px rgba(0,0,0,0.1);
  
  --radius-full: 9999px;
  --radius-xl: 28px;
  --radius-lg: 16px;

  font-family: 'Outfit', system-ui, -apple-system, sans-serif;
}

body {
  background-color: var(--md-sys-color-surface);
  color: var(--md-sys-color-on-surface);
  margin: 0;
  padding: 2rem;
  line-height: 1.6;
}

/* Typography */
h1, h2, h3 {
  font-weight: 800;
  letter-spacing: -0.02em;
  margin-bottom: 1rem;
}

h1 { font-size: 3rem; background: linear-gradient(135deg, var(--md-sys-color-primary), var(--md-sys-color-secondary)); -webkit-background-clip: text; -webkit-text-fill-color: transparent; }

/* Cards with distinct elevation */
.card {
  background: var(--md-sys-color-surface-container);
  border-radius: var(--radius-xl);
  padding: 2rem;
  box-shadow: var(--md-sys-elevation-1);
  transition: all 0.3s cubic-bezier(0.34, 1.56, 0.64, 1); /* Bouncy transition */
  border: 1px solid rgba(0,0,0,0.03);
}

.card:hover {
  transform: translateY(-4px) scale(1.01);
  box-shadow: var(--md-sys-elevation-hover);
}

/* Pill Buttons */
.btn {
  display: inline-flex;
  align-items: center;
  gap: 0.5rem;
  padding: 0.75rem 1.75rem;
  border-radius: var(--radius-full);
  background-color: var(--md-sys-color-primary);
  color: var(--md-sys-color-on-primary);
  font-weight: 600;
  text-decoration: none;
  border: none;
  cursor: pointer;
  transition: all 0.2s cubic-bezier(0.2, 0, 0, 1);
  box-shadow: 0 4px 12px rgba(var(--hue), 80%, 60%, 0.3);
}

.btn:hover {
  background-color: hsl(var(--hue), 90%, 55%);
  transform: translateY(-2px);
  box-shadow: 0 8px 16px rgba(var(--hue), 80%, 60%, 0.4);
}

.btn:active {
  transform: translateY(0) scale(0.96);
}

/* Tags/Chips */
.chip {
  display: inline-block;
  padding: 0.25rem 1rem;
  border-radius: var(--radius-full);
  background-color: var(--md-sys-color-primary-container);
  color: var(--md-sys-color-on-primary-container);
  font-size: 0.875rem;
  font-weight: 600;
}

/* Grid Layout for Reports */
.grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(300px, 1fr));
  gap: 1.5rem;
  margin-top: 2rem;
}
```

### Рекомендации по контенту
- **Google Fonts**: Подключи `<link href="https://fonts.googleapis.com/css2?family=Outfit:wght@400;600;800&display=swap" rel="stylesheet">`.
- **Иконки**: Используй Material Icons или SVG.
- **Интерактивность**: Если требуется JS, добавь скрипт для простых взаимодействий (табы, фильтрация).

Когда пользователь просит отчет, используй этот стиль по умолчанию, если не указано иное.
