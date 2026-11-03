// Plugins are looked up by name, so no line of code says which one runs.
const plugins: Record<string, () => string> = {
  summarize: () => "summary",
  translate: () => "translation",
};

export function runPlugin(name: string) {
  return plugins[name]();
}

// Nothing calls this any more.
export function legacyExport() {
  return Object.keys(plugins);
}
