import { describe, expect, it, vi } from 'vitest'
import { patchKeyedChildren, setAttrIfChanged, setTextIfChanged, syncSelectOptions } from './domPatch'

class FakeElement {
  tagName: string
  children: FakeElement[] = []
  parent: FakeElement | null = null
  attrs = new Map<string, string>()
  textContent = ''
  value = ''
  insertCalls = 0
  ownerDocument = {
    createElement: (tag: string) => new FakeElement(tag),
  }

  constructor(tag: string) {
    this.tagName = tag.toUpperCase()
  }

  get options(): FakeElement[] {
    return this.children
  }

  getAttribute(name: string): string | null {
    return this.attrs.has(name) ? this.attrs.get(name)! : null
  }

  setAttribute(name: string, value: string): void {
    this.attrs.set(name, String(value))
  }

  removeAttribute(name: string): void {
    this.attrs.delete(name)
  }

  hasAttribute(name: string): boolean {
    return this.attrs.has(name)
  }

  append(...nodes: FakeElement[]): void {
    for (const node of nodes) {
      if (node.parent) node.remove()
      node.parent = this
      this.children.push(node)
    }
  }

  insertBefore(node: FakeElement, ref: FakeElement | null): FakeElement {
    this.insertCalls += 1
    if (node.parent) node.remove()
    node.parent = this
    if (!ref) {
      this.children.push(node)
      return node
    }
    const index = this.children.indexOf(ref)
    if (index < 0) this.children.push(node)
    else this.children.splice(index, 0, node)
    return node
  }

  remove(index?: number): void {
    if (typeof index === 'number') {
      const removed = this.children.splice(index, 1)[0]
      if (removed) removed.parent = null
      return
    }
    if (!this.parent) return
    const at = this.parent.children.indexOf(this)
    if (at >= 0) this.parent.children.splice(at, 1)
    this.parent = null
  }
}

describe('keyed DOM patching', () => {
  it('updates existing keyed nodes in place without recreating or reinserting them', () => {
    const container = new FakeElement('div')
    const create = vi.fn((_item: { id: string; label: string }) => new FakeElement('button'))
    const update = (node: HTMLElement, item: { id: string; label: string }) => {
      setTextIfChanged(node, item.label)
    }

    const first = patchKeyedChildren(
      container as unknown as Element,
      [
        { id: 'a', label: '1080p · 下载' },
        { id: 'b', label: '720p · 下载' },
      ],
      item => item.id,
      create as unknown as (item: { id: string; label: string }) => HTMLElement,
      update,
    )
    expect(create).toHaveBeenCalledTimes(2)
    const insertsAfterMount = container.insertCalls

    const second = patchKeyedChildren(
      container as unknown as Element,
      [
        { id: 'a', label: '1080p · 发送中' },
        { id: 'b', label: '720p · 下载' },
      ],
      item => item.id,
      create as unknown as (item: { id: string; label: string }) => HTMLElement,
      update,
    )
    expect(create).toHaveBeenCalledTimes(2)
    expect(second[0]).toBe(first[0])
    expect(second[1]).toBe(first[1])
    expect(container.insertCalls).toBe(insertsAfterMount)
    expect(second[0].textContent).toBe('1080p · 发送中')
    expect(second[0].getAttribute('data-key')).toBe('a')
  })

  it('reorders, inserts, and removes only changed keys while preserving surviving node identity', () => {
    const container = new FakeElement('div')
    const create = () => new FakeElement('article') as unknown as HTMLElement
    const update = (node: HTMLElement, item: string) => setTextIfChanged(node, item)

    const initial = patchKeyedChildren(container as unknown as Element, ['a', 'b', 'c'], k => k, create, update)
    const [nodeA, nodeB, nodeC] = initial

    const updated = patchKeyedChildren(container as unknown as Element, ['c', 'd', 'a'], k => k, create, update)
    expect(updated[0]).toBe(nodeC)
    expect(updated[2]).toBe(nodeA)
    expect(updated[1]).not.toBe(nodeB)
    expect(container.children.map(node => node.getAttribute('data-key'))).toEqual(['c', 'd', 'a'])
  })

  it('supports animated onRemove without re-triggering removal and reclaims re-entering keys', () => {
    const container = new FakeElement('div')
    const onRemove = vi.fn((node: HTMLElement) => {
      node.setAttribute('data-leaving', '1')
    })
    const create = () => new FakeElement('div') as unknown as HTMLElement
    const update = (node: HTMLElement, item: string) => setTextIfChanged(node, item)

    const [nodeA] = patchKeyedChildren(container as unknown as Element, ['a'], k => k, create, update, { onRemove })

    // Removing 'a' invokes onRemove once and marks it leaving.
    patchKeyedChildren(container as unknown as Element, [], k => k, create, update, { onRemove })
    expect(onRemove).toHaveBeenCalledTimes(1)
    expect(nodeA.getAttribute('data-leaving')).toBe('1')

    // Subsequent empty reconcile while still leaving must not re-fire onRemove.
    patchKeyedChildren(container as unknown as Element, [], k => k, create, update, { onRemove })
    expect(onRemove).toHaveBeenCalledTimes(1)

    // Re-entering 'a' reclaims the exact same DOM node and clears data-leaving.
    const [reclaimed] = patchKeyedChildren(container as unknown as Element, ['a'], k => k, create, update, { onRemove })
    expect(reclaimed).toBe(nodeA)
    expect(reclaimed.getAttribute('data-leaving')).toBeNull()
  })

  it('patches attributes and select options in place without rebuilding the select element', () => {
    const select = new FakeElement('select')
    syncSelectOptions(
      select as unknown as HTMLSelectElement,
      [
        { value: 'auto', label: '自动（最高）' },
        { value: '0', label: '1080p · 5.2 Mbps' },
        { value: '1', label: '720p · 2.5 Mbps' },
      ],
      '0',
    )
    const optionZero = select.options[1]
    expect(select.value).toBe('0')
    expect(select.options.map(o => o.textContent)).toEqual(['自动（最高）', '1080p · 5.2 Mbps', '720p · 2.5 Mbps'])

    // Same options + new selection keeps every <option> object intact.
    syncSelectOptions(
      select as unknown as HTMLSelectElement,
      [
        { value: 'auto', label: '自动（最高）' },
        { value: '0', label: '1080p · 5.2 Mbps' },
        { value: '1', label: '720p · 2.5 Mbps' },
      ],
      '1',
    )
    expect(select.options[1]).toBe(optionZero)
    expect(select.value).toBe('1')

    // Shrinking options trims only excess nodes and updates text in place.
    syncSelectOptions(
      select as unknown as HTMLSelectElement,
      [
        { value: 'auto', label: '自动（最高）' },
        { value: '0', label: '1080p · 6.0 Mbps' },
      ],
      'auto',
    )
    expect(select.options).toHaveLength(2)
    expect(select.options[1]).toBe(optionZero)
    expect(optionZero.textContent).toBe('1080p · 6.0 Mbps')

    setAttrIfChanged(select as unknown as Element, 'aria-busy', 'true')
    expect(select.getAttribute('aria-busy')).toBe('true')
    setAttrIfChanged(select as unknown as Element, 'aria-busy', null)
    expect(select.hasAttribute('aria-busy')).toBe(false)
  })
})
