#!/usr/bin/env python3
"""Optional Chromium typography/layout smoke test; requires Python Playwright.

    python3 scripts/web-typography-smoke.py
    python3 scripts/web-typography-smoke.py --base-url http://127.0.0.1:3010
    python3 scripts/web-typography-smoke.py --screenshot-dir /existing/directory

Loads CSS in index.html order into synthetic session/Life DOM, without running
the app or calling its APIs. Deployed mode fetches only the public index and
same-origin CSS assets. External font stylesheets are skipped and browser
network requests are blocked, so layout uses the CSS system-font fallbacks.
Screenshots are opt-in; their directory must already exist and not be in /tmp.
Failures are collected across every viewport and produce a nonzero exit code.
"""

import argparse
from html import escape
from html.parser import HTMLParser
from pathlib import Path
import sys
from urllib.parse import unquote, urljoin, urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener


class StylesheetParser(HTMLParser):
    """Preserve the document's link/style cascade order and media conditions."""

    def __init__(self):
        super().__init__()
        self.entries = []
        self.base_href = None
        self.inline = None

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if tag == "base" and self.base_href is None:
            self.base_href = attrs.get("href")
        if tag == "link" and "disabled" not in attrs:
            rel = (attrs.get("rel") or "").lower().split()
            # Trunk's source uses rel="css"; built assets use rel="stylesheet".
            if "stylesheet" in rel or ("css" in rel and "data-trunk" in attrs):
                href = attrs.get("href")
                if not href:
                    raise ValueError("Stylesheet link has no href")
                self.entries.append(("link", href, attrs.get("media") or "all"))
        if tag == "style":
            self.inline = [attrs.get("media") or "all", []]

    def handle_data(self, data):
        if self.inline is not None:
            self.inline[1].append(data)

    def handle_endtag(self, tag):
        if tag == "style" and self.inline is not None:
            media, chunks = self.inline
            self.entries.append(("inline", "".join(chunks), media))
            self.inline = None


def origin(url):
    parts = urlsplit(url)
    if parts.scheme not in {"http", "https"} or not parts.hostname:
        raise ValueError(f"Expected an HTTP(S) URL: {url}")
    if parts.username is not None or parts.password is not None:
        raise ValueError("URLs containing credentials are not supported")
    return parts.scheme, parts.hostname, parts.port or (443 if parts.scheme == "https" else 80)


class PublicAssetRedirects(HTTPRedirectHandler):
    def __init__(self, asset_origin):
        super().__init__()
        self.asset_origin = asset_origin

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        # Never follow a stylesheet/index redirect to an API or login endpoint.
        path = unquote(urlsplit(newurl).path)
        if origin(newurl) != self.asset_origin or not (
            path.endswith(".css") or path.endswith("/index.html") or path.endswith("/")
        ) or "/api/" in path:
            raise ValueError(f"Refusing non-public-asset redirect: {newurl}")
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def load_styles(base_url):
    root = Path(__file__).resolve().parents[1] / "crates/oxide-agent-web-ui"
    if base_url:
        asset_origin = origin(base_url)
        parts = urlsplit(base_url)
        if parts.query or parts.fragment:
            raise ValueError("--base-url must not include a query or fragment")
        index_url = base_url.rstrip("/") + "/"
        opener = build_opener(PublicAssetRedirects(asset_origin))

        def fetch(url, css=False):
            path = unquote(urlsplit(url).path)
            if origin(url) != asset_origin or "/api/" in path:
                raise ValueError(f"Refusing non-public asset: {url}")
            if css and not path.endswith(".css"):
                raise ValueError(f"Expected a public .css asset: {url}")
            with opener.open(Request(url, headers={"User-Agent": "OxideTypographySmoke/1"}), timeout=20) as response:
                content_type = response.headers.get_content_type()
                if css and content_type != "text/css":
                    raise ValueError(f"Expected text/css for {url}, got {content_type}")
                return response.read().decode(response.headers.get_content_charset() or "utf-8"), response.url

        index, index_url = fetch(index_url)
        source = index_url
    else:
        index = (root / "index.html").read_text(encoding="utf-8")
        source = str(root / "index.html")

    parser = StylesheetParser()
    parser.feed(index)
    parser.close()
    styles = []
    skipped = []
    for kind, value, media in parser.entries:
        if kind == "inline":
            styles.append(("inline style", value, media))
            continue
        if base_url:
            url = urljoin(urljoin(index_url, parser.base_href or ""), value)
            if origin(url) != asset_origin:
                skipped.append(url)
                continue
            css, _ = fetch(url, css=True)
            label = url
        else:
            parts = urlsplit(value)
            if parts.scheme or parts.netloc:
                skipped.append(value)
                continue
            path = (root / unquote(parts.path)).resolve()
            if not path.is_relative_to(root.resolve()):
                raise ValueError(f"Stylesheet escapes UI source directory: {value}")
            css = path.read_text(encoding="utf-8")
            label = value
        styles.append((label, css, media))
    if not styles:
        raise ValueError(f"No application stylesheets found in {source}")
    return source, styles, skipped


PARAGRAPH = (
    "Длинный ответ на русском языке должен оставаться удобным для чтения: "
    "достаточный размер шрифта, спокойный межстрочный интервал и ограниченная "
    "ширина строки помогают проверять результаты работы агента. "
) * 5
LONG_URL = "https://example.org/" + "очень-длинный-путь-без-пробелов/" * 18
CODE = 'fn main() {\n    let message = "' + "wide_code_without_spaces_" * 90 + '";\n\tprintln!("{}", message);\n}\n'


def markdown_fixture():
    # Match comrak output and markdown.rs's add_code_copy_buttons wrapper.
    # 36 cells' padding/borders alone exceed 768px, even when every word wraps.
    # Fewer columns can legitimately fit and would not exercise scroll containment.
    headers = "".join(f"<th>Длинный заголовок столбца {n}</th>" for n in range(36))
    cells = "".join(f"<td>{escape(PARAGRAPH)} {escape(LONG_URL)}</td>" for _ in range(36))
    return f"""
      <p data-prose>{escape(PARAGRAPH)}</p>
      <h1>Заголовок первого уровня</h1><h2>Заголовок второго уровня</h2>
      <h3>Заголовок третьего уровня</h3>
      <p>Встроенный код <code data-inline-code>let значение = 42;</code>.</p>
      <ul data-list><li>Первый пункт<ul data-nested-list><li>Вложенный пункт</li>
        <li>Второй вложенный пункт</li></ul></li><li>Второй пункт</li></ul>
      <ol data-loose-list><li><p>Первый абзац свободного списка.</p>
        <p>Продолжение первого пункта.</p></li><li><p>Второй пункт.</p></li></ol>
      <p><a data-long-url href="{escape(LONG_URL, quote=True)}">{escape(LONG_URL)}</a></p>
      <blockquote><p>{escape(PARAGRAPH)}</p></blockquote>
      <table><thead><tr>{headers}</tr></thead><tbody><tr>{cells}</tr></tbody></table>
      <div class="code-block"><button class="code-copy-button" type="button"
        data-copy-code="true">Copy</button><pre><code>{escape(CODE)}</code></pre></div>
    """


def attachments():
    return """<ul class="message-attachments"><li class="message-attachment-item">
      <div class="message-attachment-copy"><span class="message-attachment-name">отчёт.txt</span>
      <span class="message-attachment-meta">text/plain · 1234 bytes</span></div>
      <code class="message-attachment-path">/workspace/отчёт.txt</code></li></ul>"""


def fixture(mode, drawer_open):
    composer = f"""<form class="composer {'life-composer' if mode == 'life' else ''}">
      <div class="composer-inner"><textarea rows="2" aria-label="Message">Проверка типографики</textarea>
      <div class="composer-footer"><div class="composer-actions">
        <button type="button" class="composer-attach-button">+</button>
        <button type="button" class="btn-primary">Send</button></div></div></div></form>"""
    drawer = f"""<aside class="activity-drawer {'open' if drawer_open else ''}">
      <div class="activity-header"><span class="activity-title">Activity</span></div>
      <div class="activity-timeline">Representative activity</div></aside>"""
    markdown = markdown_fixture()
    if mode == "session":
        workspace = f"""<section class="session-workspace"><div class="chat-wrapper">
          <div class="results-panel"><article class="task-card">
          <div class="message user-message-wrap"><div class="user-message">
            <div class="user-message-body"><div class="message-collapsible">
              <div class="message-collapsible-body is-expanded"><div class="markdown-content" data-markdown="user">
                <p data-prose>{escape(PARAGRAPH)}</p><p>{escape(LONG_URL)}</p>
              </div></div></div>{attachments()}</div></div></div>
          <div class="task-action-row"><button class="thinking-button">Activity</button></div>
          <div class="message assistant-message-wrap"><div class="assistant-message">
            <div class="markdown-content" data-markdown="assistant">{markdown}</div></div></div>
          <div class="assistant-progress-log"><div class="message assistant-progress-block">
            <div class="assistant-progress-headline"><div class="markdown-content"><p>Проверка результата</p></div></div>
            <div class="assistant-progress-detail">Подробности проверки результата</div>
          </div></div></article></div>{composer}</div>{drawer}</section>"""
    else:
        # life-console is the actual app class; life-workspace labels the fixture.
        workspace = f"""<section class="life-console life-workspace"><div class="life-chat-wrapper">
          <div class="life-results-panel"><div class="life-transcript">
          <div class="life-turn user"><div class="life-turn-header"><span class="life-turn-role">You</span></div>
            <div class="life-turn-content" data-life-user>{escape(PARAGRAPH)}<br>{escape(LONG_URL)}</div>
            {attachments()}</div>
          <div class="life-turn assistant"><div class="life-turn-header"><span class="life-turn-role">Agent</span></div>
            <div class="life-turn-content markdown-content" data-markdown="assistant">{markdown}</div>
          </div></div></div>{composer}</div>{drawer}</section>"""
    return f"""<!doctype html><html lang="ru"><head><meta charset="utf-8">
      <meta name="viewport" content="width=device-width, initial-scale=1"></head><body>
      <main id="app"><div class="app-layout"><aside class="sidebar">
        <div class="sidebar-header"><h2>Oxide Agent</h2></div></aside>
        <main class="workspace-main">{workspace}</main></div></main></body></html>"""


INSPECT = r"""({mode, drawerOpen, expectedCode}) => {
    const failures = [];
    let checks = 0;
    function check(ok, message) { checks++; if (!ok) failures.push(message); }
    const near = (a, b) => Math.abs(a - b) <= 0.1;
    function element(selector) {
        const el = document.querySelector(selector);
        if (!el) throw new Error(`Fixture missing ${selector}`);
        return el;
    }
    function number(selector, property, expected) {
        const actual = parseFloat(getComputedStyle(element(selector))[property]);
        check(near(actual, expected), `${selector} ${property}: expected ${expected}px, got ${actual}px`);
    }
    function typography(selector, size, lineHeight) {
        number(selector, 'fontSize', size);
        number(selector, 'lineHeight', lineHeight);
    }
    const assistant = '[data-markdown="assistant"]';
    const panel = mode === 'session' ? '.results-panel' : '.life-results-panel';
    const column = mode === 'session' ? '.task-card' : '.life-transcript';
    if (mode === 'session') {
        for (const selector of ['.message', '.user-message', '.assistant-message',
                                '[data-markdown="user"] [data-prose]',
                                '.assistant-progress-headline']) {
            typography(selector, 16, 26);
        }
        // Technical progress metadata stays compact; it is not chat prose.
        typography('.assistant-progress-detail', 12, 18);
    } else {
        for (const selector of ['.life-turn.user', '.life-turn.assistant', '[data-life-user]']) {
            typography(selector, 16, 26);
        }
    }
    typography(`${assistant} [data-prose]`, 16, 26);
    typography('.composer textarea', 16, 26);
    typography('.message-attachment-item', 13, 19.5);
    number('.message-attachment-name', 'fontSize', 13);
    for (const [tag, size] of [['h1', 28], ['h2', 24], ['h3', 20]]) {
        typography(`${assistant} ${tag}`, size, size * 1.3);
    }
    typography(`${assistant} th`, 14, 22.4);
    typography(`${assistant} td`, 14, 22.4);
    number(`${assistant} [data-inline-code]`, 'fontSize', 14);
    typography(`${assistant} pre code`, 14, 22.4);
    const code = element(`${assistant} pre code`);
    check(code.textContent === expectedCode, 'Code text lost tabs/newlines/indentation');
    for (const selector of [`${assistant} pre`, `${assistant} pre code`]) {
        const style = getComputedStyle(element(selector));
        check(['pre', 'break-spaces'].includes(style.whiteSpace),
              `${selector} must preserve whitespace without wrapping, got ${style.whiteSpace}`);
    }
    // Life's word-break can override white-space/overflow-wrap and silently wrap code.
    const codeRange = document.createRange();
    const text = code.firstChild;
    const firstLine = expectedCode.indexOf('\n') + 1;
    codeRange.setStart(text, firstLine);
    codeRange.setEnd(text, expectedCode.indexOf('\n', firstLine));
    const lineRects = [...codeRange.getClientRects()];
    check(lineRects.length > 0 && lineRects.every(rect => near(rect.top, lineRects[0].top)),
          `Wide code line wraps into ${lineRects.length} visual fragments`);
    function scrollContained(selector) {
        const el = element(selector);
        const style = getComputedStyle(el);
        check(['auto', 'scroll'].includes(style.overflowX),
              `${selector} overflow-x must allow scrolling, got ${style.overflowX}`);
        check(el.scrollWidth > el.clientWidth + 1,
              `${selector} wide fixture must overflow internally (${el.scrollWidth}/${el.clientWidth})`);
        el.scrollLeft = 80;
        check(el.scrollLeft > 0, `${selector} cannot be horizontally scrolled`);
        el.scrollLeft = 0;
        const bounds = el.getBoundingClientRect();
        const parent = element(assistant).getBoundingClientRect();
        check(bounds.left >= parent.left - 1 && bounds.right <= parent.right + 1,
              `${selector} escapes Markdown column (${bounds.width.toFixed(1)} vs ${parent.width.toFixed(1)}px)`);
    }
    scrollContained(`${assistant} pre code`);
    scrollContained(`${assistant} table`);
    for (const list of ['[data-list]', '[data-nested-list]', '[data-loose-list]']) {
        number(`${assistant} ${list}`, 'paddingLeft', 24);
        number(`${assistant} ${list} > li + li`, 'marginTop', 6);
        const el = element(`${assistant} ${list}`);
        const parentLeft = el.getBoundingClientRect().left;
        check(el.firstElementChild.getBoundingClientRect().left >= parentLeft + 23,
              `${list} list-item indentation is missing`);
    }
    number(`${assistant} [data-nested-list]`, 'marginTop', 8);
    number(`${assistant} [data-loose-list] > li > p`, 'marginBottom', 8);
    number(`${assistant} [data-loose-list] > li > p:last-child`, 'marginBottom', 0);
    const url = element(`${assistant} [data-long-url]`);
    const urlRange = document.createRange();
    urlRange.selectNodeContents(url);
    const markdownBounds = element(assistant).getBoundingClientRect();
    check([...urlRange.getClientRects()].every(rect => rect.left >= markdownBounds.left - 1 &&
          rect.right <= markdownBounds.right + 1), 'Long URL escapes the Markdown column');
    const available = element(panel).clientWidth - parseFloat(getComputedStyle(element(panel)).paddingLeft)
        - parseFloat(getComputedStyle(element(panel)).paddingRight);
    const expectedWidth = Math.min(768, available);
    for (const selector of [column, mode === 'session' ? '.user-message' : '.life-turn.user',
                           mode === 'session' ? '.assistant-message' : '.life-turn.assistant']) {
        const actual = element(selector).getBoundingClientRect().width;
        check(Math.abs(actual - expectedWidth) <= 1,
              `${selector} width: expected ${expectedWidth}px, got ${actual.toFixed(1)}px`);
    }
    number(column, 'maxWidth', 768);
    number('.composer', 'maxWidth', 768);
    const composer = element('.composer').getBoundingClientRect();
    const wrapper = element(mode === 'session' ? '.chat-wrapper' : '.life-chat-wrapper');
    check(Math.abs(composer.width - Math.min(768, wrapper.clientWidth - 40)) <= 1,
          `Composer width ${composer.width.toFixed(1)}px does not match available width / 768px cap`);
    const layoutSelectors = ['html', 'body', '.app-layout', '.workspace-main', panel, column,
        '.composer', '.composer-inner', assistant, `${assistant} .code-block`, `${assistant} pre`,
        mode === 'session' ? '.session-workspace' : '.life-console',
        mode === 'session' ? '.chat-wrapper' : '.life-chat-wrapper'];
    for (const selector of layoutSelectors) {
        const el = element(selector);
        check(el.scrollWidth <= el.clientWidth + 1,
              `${selector} horizontal overflow: scrollWidth=${el.scrollWidth}, clientWidth=${el.clientWidth}`);
        const rect = el.getBoundingClientRect();
        check(rect.left >= -1 && rect.right <= innerWidth + 1,
              `${selector} extends outside viewport: left=${rect.left.toFixed(1)}, right=${rect.right.toFixed(1)}`);
    }
    const drawer = element('.activity-drawer');
    check(near(drawer.getBoundingClientRect().width, drawerOpen ? 400 : 0),
          `Activity drawer width should be ${drawerOpen ? 400 : 0}px`);
    check(getComputedStyle(element('.sidebar')).display === (innerWidth <= 1024 ? 'none' : 'flex'),
          'Sidebar visibility does not match the responsive breakpoint');
    return {checks, failures};
}"""


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--base-url", help="Read deployed public index/CSS instead of local source CSS")
    parser.add_argument("--screenshot-dir", type=Path, help="Existing screenshot directory outside /tmp")
    args = parser.parse_args()
    if args.screenshot_dir:
        args.screenshot_dir = args.screenshot_dir.resolve()
        if not args.screenshot_dir.is_dir() or args.screenshot_dir == Path("/tmp") or Path("/tmp") in args.screenshot_dir.parents:
            parser.error("--screenshot-dir must be an existing directory outside /tmp")
    try:
        from playwright.sync_api import sync_playwright

        source, styles, skipped = load_styles(args.base_url)
        print(f"CSS source: {source}")
        print("Cascade: " + " -> ".join(label for label, _, _ in styles))
        if skipped:
            print("Skipped external stylesheets (system-font fallbacks): " + ", ".join(skipped))
        failed = 0
        total_checks = 0
        cases = [(1920, 1080, False), (1024, 768, False), (390, 844, False), (320, 740, False),
                 (1920, 1080, True), (1024, 768, True)]
        with sync_playwright() as playwright:
            # Use bundled Chromium rather than the separately installed headless shell.
            executable = Path(playwright.chromium.executable_path)
            if not executable.is_file():
                # Python and Node Playwright may expect different cached revisions.
                cached = list(executable.parents[2].glob("chromium-*/chrome-linux*/chrome"))
                if not cached:
                    raise RuntimeError("No bundled Chromium found; install a Playwright Chromium browser")
                executable = max(cached, key=lambda path: path.stat().st_mtime)
            print(f"Chromium: {executable}")
            browser = playwright.chromium.launch(headless=True, executable_path=str(executable))
            context = browser.new_context(device_scale_factor=1, reduced_motion="reduce")
            context.route("**/*", lambda route: route.abort())
            for mode in ("session", "life"):
                for width, height, drawer_open in cases:
                    label = f"{mode}-{width}x{height}-drawer-{'open' if drawer_open else 'closed'}"
                    page = context.new_page()
                    page.set_viewport_size({"width": width, "height": height})
                    page.set_content(fixture(mode, drawer_open))
                    for _, css, media in styles:
                        tag = page.add_style_tag(content=css)
                        tag.evaluate("(tag, media) => tag.media = media", media)
                    page.evaluate("() => document.fonts.ready")
                    page.evaluate("() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))")
                    result = page.evaluate(INSPECT, {"mode": mode, "drawerOpen": drawer_open, "expectedCode": CODE})
                    total_checks += result["checks"]
                    failures = result["failures"]
                    failed += len(failures)
                    print(f"{'FAIL' if failures else 'PASS'} {label}: {result['checks']} checks, {len(failures)} failures")
                    for failure in failures:
                        print(f"  - {failure}")
                    if args.screenshot_dir:
                        page.screenshot(path=str(args.screenshot_dir / f"{label}.png"), full_page=True)
                    page.close()
            browser.close()
        print(f"Result: {total_checks} checks across {len(cases) * 2} cases; {failed} failures")
        return 1 if failed else 0
    except Exception as error:
        print(f"Typography smoke error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
