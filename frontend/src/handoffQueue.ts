/** Keep one native confirmation window focused while later offers wait. */
export class HandoffWindowQueue {
  private active = ''
  private waiting: string[] = []

  enqueue(id: string): boolean {
    if (!id || id === this.active || this.waiting.includes(id)) return false
    this.waiting.push(id)
    return true
  }

  begin(): string {
    if (this.active) return ''
    this.active = this.waiting.shift() || ''
    return this.active
  }

  release(id: string): boolean {
    if (!id || id !== this.active) return false
    this.active = ''
    return true
  }

  get activeId(): string {
    return this.active
  }
}
