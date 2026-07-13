import React from 'react'

export default function ProgressBar({ percent, status }: { percent: number; status?: string }) {
  const p = Math.max(0, Math.min(100, percent))
  const isDone = status === 'done'
  const isFailed = status === 'failed'
  const isMerging = status === 'merging'
  const isRemuxing = status === 'remuxing'

  let bg: string
  let anim = false
  if (isFailed) {
    bg = 'linear-gradient(90deg, #ef4444, #f97316)'
  } else if (isMerging) {
    bg = 'linear-gradient(90deg, #f59e0b, #fbbf24)'
    anim = true
  } else if (isRemuxing) {
    bg = 'linear-gradient(90deg, #a855f7, #6366f1)'
    anim = true
  } else if (isDone) {
    bg = 'linear-gradient(90deg, #16a34a, #22c55e)'
  } else {
    bg = 'linear-gradient(90deg, #3b82f6, #60a5fa)'
    anim = true
  }

  const label = isDone ? '100%' : isMerging ? `合并 ${p.toFixed(1)}%` : isRemuxing ? `转封装 ${p.toFixed(1)}%` : `${p.toFixed(1)}%`

  return (
    <div style={{ position: 'relative', height: 22, background: '#1e293b', borderRadius: 11, overflow: 'hidden' }}>
      <div style={{
        height: '100%',
        width: `${p}%`,
        background: bg,
        borderRadius: 11,
        transition: 'width 0.4s ease',
        ...(anim ? { backgroundSize: '200% 100%', animation: 'hls-shimmer 1.5s infinite linear' } : {}),
      }} />
      <span style={{
        position: 'absolute', inset: 0,
        display: 'flex', alignItems: 'center', justifyContent: 'center',
        fontSize: 11, fontWeight: 700, color: '#fff',
        textShadow: '0 1px 3px rgba(0,0,0,0.6)',
        letterSpacing: 0.5,
      }}>{label}</span>
      <style>{`
        @keyframes hls-shimmer {
          0% { background-position: 200% 0; }
          100% { background-position: -200% 0; }
        }
      `}</style>
    </div>
  )
}
