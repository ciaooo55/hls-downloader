import { describe, expect, it } from 'vitest'

import { getFailureDetails } from './failureDetails'

describe('failure details', () => {
  it('formats structured downloader diagnostics for display', () => {
    const details = getFailureDetails({
      error_code: 'HTTP_403',
      error_stage: 'downloading_segments',
      error_url: 'https://cdn.example.test/1.ts',
      error_hint: '检查 Referer、Origin 和 Cookie',
      error_message: '[HTTP_403] HTTP 403 Forbidden',
      http_status: 403,
      error_attempt: 5,
    })

    expect(details.title).toBe('下载失败 · HTTP_403')
    expect(details.items).toEqual([
      { label: '发生阶段', value: '下载分片' },
      { label: 'HTTP 状态', value: '403' },
      { label: '错误代码', value: 'HTTP_403' },
      { label: '尝试次数', value: '5 次' },
      { label: '资源地址', value: 'https://cdn.example.test/1.ts' },
    ])
    expect(details.message).toContain('403')
    expect(details.hint).toContain('Referer')
    expect(details.steps?.length).toBeGreaterThan(0)
    expect(details.steps?.some(step => step.includes('扩展'))).toBe(true)
  })

  it('keeps old task history readable when only error_message exists', () => {
    const details = getFailureDetails({ error_message: 'legacy failure' })

    expect(details.title).toBe('下载失败')
    expect(details.items).toEqual([])
    expect(details.message).toBe('legacy failure')
    expect(details.hint).toBe('')
  })
})
