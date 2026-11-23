/**
 * Keyed in-place DOM reconciliation utilities for extension overlays and popup.
 *
 * Updating existing nodes by `data-key` preserves keyboard focus, hover state,
 * `<details>` expansion, open `<select>` dropdowns, and in-flight CSS
 * transitions without `replaceChildren()` flicker.
 */

export interface PatchKeyedOptions<E extends HTMLElement> {
  /** Return true when an existing child belongs to this managed keyed set. */
  match?: (node: Element) => node is E
  /** Optional custom removal (e.g. for animated exit before detaching). */
  onRemove?: (node: E, key: string) => void
}

export function setTextIfChanged(node: Node, text: string): void {
  if (node.textContent !== text) node.textContent = text
}

export function setAttrIfChanged(node: Element, name: string, value: string | null): void {
  if (value === null) {
    if (node.hasAttribute(name)) node.removeAttribute(name)
  } else if (node.getAttribute(name) !== value) {
    node.setAttribute(name, value)
  }
}

export function patchKeyedChildren<T, E extends HTMLElement>(
  container: Element,
  items: readonly T[],
  keyOf: (item: T) => string,
  create: (item: T, key: string) => E,
  update: (node: E, item: T, key: string) => void,
  options?: PatchKeyedOptions<E>,
): E[] {
  const existing = new Map<string, E>()
  const unmanaged: Element[] = []
  for (const child of Array.from(container.children)) {
    const key = child.getAttribute('data-key')
    if (key && (!options?.match || options.match(child))) {
      if (!existing.has(key)) existing.set(key, child as E)
      else child.remove()
    } else if (child.getAttribute('data-leaving') !== '1') {
      unmanaged.push(child)
    }
  }
  for (const stale of unmanaged) stale.remove()

  const ordered: E[] = []
  const seenKeys = new Set<string>()
  for (const item of items) {
    const key = keyOf(item)
    if (seenKeys.has(key)) continue
    seenKeys.add(key)
    let node = existing.get(key)
    if (node) {
      existing.delete(key)
      if (node.getAttribute('data-leaving') === '1') node.removeAttribute('data-leaving')
      update(node, item, key)
    } else {
      node = create(item, key)
      node.setAttribute('data-key', key)
      update(node, item, key)
    }
    ordered.push(node)
  }

  for (const [key, stale] of existing) {
    if (stale.getAttribute('data-leaving') === '1') continue
    if (options?.onRemove) options.onRemove(stale, key)
    else stale.remove()
  }

  let cursorIndex = 0
  for (const node of ordered) {
    let cursor = container.children[cursorIndex] as Element | undefined
    while (cursor && cursor.getAttribute('data-leaving') === '1') {
      cursorIndex += 1
      cursor = container.children[cursorIndex] as Element | undefined
    }
    if (cursor !== node) {
      container.insertBefore(node, cursor || null)
    }
    cursorIndex += 1
  }

  return ordered
}

export interface SelectOptionEntry {
  value: string
  label: string
}

/** Update `<option>` entries and selection in place without rebuilding `<select>`. */
export function syncSelectOptions(
  select: HTMLSelectElement,
  options: readonly SelectOptionEntry[],
  selectedValue: string,
): void {
  const current = Array.from(select.options)
  const sameOptions = current.length === options.length
    && current.every((opt, index) => opt.value === options[index].value && opt.textContent === options[index].label)
  if (!sameOptions) {
    while (select.options.length > options.length) {
      select.remove(select.options.length - 1)
    }
    for (let index = 0; index < options.length; index += 1) {
      const entry = options[index]
      const existing = select.options[index]
      if (existing) {
        if (existing.value !== entry.value) existing.value = entry.value
        setTextIfChanged(existing, entry.label)
      } else {
        const option = select.ownerDocument.createElement('option')
        option.value = entry.value
        option.textContent = entry.label
        select.append(option)
      }
    }
  }
  if (select.value !== selectedValue) {
    select.value = selectedValue
  }
}
