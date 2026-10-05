// Per-viewer UI preferences. Every access is guarded: storage can be absent.

export function load<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(`trav.ui.${key}`);
    return raw == null ? fallback : (JSON.parse(raw) as T);
  } catch {
    return fallback;
  }
}

export function save(key: string, value: unknown) {
  try {
    localStorage.setItem(`trav.ui.${key}`, JSON.stringify(value));
  } catch {
    /* ignore */
  }
}
