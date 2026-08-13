import { fmtBytes } from '../format'
import { buildConnectionMap } from '../connectionMap'

const STATE_LABEL = {
  done: '\u5df2\u5b8c\u6210',
  active: '\u4e0b\u8f7d\u4e2d',
  queued: '\u672a\u5f00\u59cb',
}

export default function ConnectionMap({
  parts,
  total = 0,
  compact = false,
}: {
  parts?: unknown
  total?: number
  compact?: boolean
}) {
  const model = buildConnectionMap(parts, total)
  if (!model) return null
  const summary = compact
    ? `${model.active} \u6761\u6d3b\u52a8\u8fde\u63a5`
    : `${model.parts.length} \u6bb5 \u00b7 ${model.active} \u6761\u6d3b\u52a8\u8fde\u63a5 \u00b7 ${fmtBytes(model.doneBytes)} / ${fmtBytes(model.total)}`
  return (
    <div className={compact ? 'connection-map is-compact' : 'connection-map'} title={summary} role="img" aria-label="\u5206\u6bb5\u8fde\u63a5">
      <div className="connection-map-bar">
        {model.parts.map((part) => (
          <i
            key={`${part.start}-${part.end}-${part.state}`}
            className={`is-${part.state}`}
            style={{ flexGrow: Math.max(part.flex, 1), flexBasis: 0 }}
            title={`${STATE_LABEL[part.state]} \u00b7 ${fmtBytes(part.start)}-${fmtBytes(part.end)}`}
          >
            {part.state === 'active' && part.fill < 100 ? <b style={{ width: `${part.fill}%` }} /> : null}
          </i>
        ))}
      </div>
      {!compact && <small>{summary}</small>}
    </div>
  )
}
