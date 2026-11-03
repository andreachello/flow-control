// A tiny event bus. Handlers are looked up by name at runtime.
const handlers: Record<string, Array<() => void>> = {};

export function on(evt: string, fn: () => void) {
  handlers[evt] = [...(handlers[evt] ?? []), fn];
}

export function emit(evt: string) {
  (handlers[evt] ?? []).forEach((fn) => fn());
}
