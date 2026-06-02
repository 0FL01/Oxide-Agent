# Oxide Agent — Brand Spec

Dark neutral ChatGPT-inspired coding-agent console. Branded as Oxide Agent, not a ChatGPT clone.

## Color Tokens (OKLch)

```css
:root {
  --bg:      oklch(24% 0 0);    /* #171717 — page background */
  --surface: oklch(28% 0 0);    /* #212121 — elevated surfaces, inputs, tool cards */
  --fg:      oklch(92% 0 0);    /* #ececec — primary text */
  --muted:   oklch(60% 0.012 280); /* #8e8e93 — secondary text */
  --border:  oklch(33% 0 0);    /* #2a2a2a — dividers, subtle borders */
  --accent:  oklch(63% 0.15 170); /* #10a37f — running/success/link */

  /* Extended palette */
  --surface-hover: oklch(36% 0 0);    /* #2f2f2f */
  --border-strong: oklch(43% 0 0);    /* #3a3a3a */
  --text-faint:    oklch(50% 0.012 280); /* #6e6e73 */
  --warning:       oklch(70% 0.14 85);  /* #d8a21e */
  --error:         oklch(60% 0.20 25);  /* #ef4444 */
  --accent-glow:   rgba(16,163,127,0.18);
}
```

## Typography

- **Display / UI**: -apple-system, BlinkMacSystemFont, 'SF Pro Display', 'Inter', system-ui, sans-serif
- **Body**: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Inter', system-ui, sans-serif
- **Mono (data, timestamps, tool names, IDs)**: 'JetBrains Mono', 'SF Mono', ui-monospace, Menlo, monospace
- **Base size**: 13px, line-height 1.5
- **Antialiased**: -webkit-font-smoothing: antialiased

## Layout Posture

- Soft rounded corners: 8–14px for composer, panels, cards, inputs. NOT brutalist zero-radius.
- Subtle 1px borders only (`--border`, `--border-strong`). No strong shadows, no elevation.
- Depth via background color shifts only (`--bg` → `--surface` → `--surface-hover`).
- One accent color (`--accent`) used at most twice per screen for primary actions and running indicators.
- Monospace reserved for command names, IDs, timestamps, metrics, logs — never for body copy.
- Minimal top chrome; content-led layouts.
- Generous whitespace in chat column; dense telemetry in Activity drawer.
