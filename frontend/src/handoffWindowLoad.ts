export interface HandoffPaintItem {
  id: string
  status?: string
}

export function pendingHandoffCount(handoffId: string, pending: HandoffPaintItem[]): number {
  return Math.max(0, pending.filter(item => item.id !== handoffId && item.status === 'pending').length)
}

/** Paint the confirmation dialog from the offer first; settings/queue fill in after. */
export async function loadHandoffPresentation<T extends HandoffPaintItem, S>(
  handoffId: string,
  io: {
    fetchHandoff: (id: string) => Promise<T>
    fetchSettings: () => Promise<S>
    fetchHandoffs: () => Promise<T[]>
  },
  emit: {
    item: (item: T) => void
    extras: (state: { settings: S; queueRemaining: number }) => void
  },
): Promise<{ close: boolean }> {
  const handoff = await io.fetchHandoff(handoffId)
  if (handoff.status && handoff.status !== 'pending') return { close: true }
  emit.item(handoff)
  const [settings, pending] = await Promise.all([io.fetchSettings(), io.fetchHandoffs()])
  emit.extras({
    settings,
    queueRemaining: pendingHandoffCount(handoffId, pending),
  })
  return { close: false }
}
