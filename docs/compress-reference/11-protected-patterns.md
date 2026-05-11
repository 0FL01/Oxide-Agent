# 11. Protected Patterns (Glob Matching)

> Source: `lib/protected-patterns.ts`

---

## Glob matching

```typescript
// lib/protected-patterns.ts

export function matchesGlob(inputPath: string, pattern: string): boolean {
    if (!pattern) return false

    const input = inputPath.replaceAll("\\\\", "/")
    const pat = pattern.replaceAll("\\\\", "/")

    let regex = "^"
    for (let i = 0; i < pat.length; i++) {
        const ch = pat[i]
        if (ch === "*") {
            const next = pat[i + 1]
            if (next === "*") {
                const after = pat[i + 2]
                if (after === "/") {
                    regex += "(?:.*/)?"
                    i += 2
                    continue // **/ = zero+ dirs
                }
                regex += ".*"
                i++
                continue // ** = anything
            }
            regex += "[^/]*"
            continue // * = no slashes
        }
        if (ch === "?") {
            regex += "[^/]"
            continue
        } // ? = single char
        if (ch === "/") {
            regex += "/"
            continue
        }
        regex += /[\\.^$+{}()|\[\]]/.test(ch) ? `\\${ch}` : ch
    }
    regex += "$"
    return new RegExp(regex).test(input)
}
```

## File path extraction from tool parameters

```typescript
export function getFilePathsFromParameters(tool: string, parameters: unknown): string[] {
    if (typeof parameters !== "object" || parameters === null) return []
    const params = parameters as Record<string, any>
    const paths: string[] = []

    // apply_patch: parse from patchText
    if (tool === "apply_patch" && typeof params.patchText === "string") {
        const pathRegex = /\*\*\* (?:Add|Delete|Update) File: ([^\n\r]+)/g
        let match
        while ((match = pathRegex.exec(params.patchText)) !== null) paths.push(match[1].trim())
    }

    // multiedit: top-level + nested edits
    if (tool === "multiedit") {
        if (typeof params.filePath === "string") paths.push(params.filePath)
        if (Array.isArray(params.edits))
            for (const edit of params.edits) if (edit?.filePath) paths.push(edit.filePath)
    }

    // Default: filePath parameter
    if (typeof params.filePath === "string") paths.push(params.filePath)

    return [...new Set(paths)].filter((p) => p.length > 0)
}
```

## Protection checks

```typescript
export function isFilePathProtected(filePaths: string[], patterns: string[]): boolean {
    if (!filePaths?.length || !patterns?.length) return false
    return filePaths.some((path) => patterns.some((pattern) => matchesGlob(path, pattern)))
}

export function isToolNameProtected(toolName: string, patterns: string[]): boolean {
    if (!toolName || !patterns?.length) return false
    const exact = new Set<string>()
    const globs: string[] = []
    for (const p of patterns) {
        if (/[*?]/.test(p)) globs.push(p)
        else exact.add(p)
    }
    return exact.has(toolName) || globs.some((p) => matchesGlob(toolName, p))
}
```
