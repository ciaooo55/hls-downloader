import { describe, expect, it } from 'vitest'
import { classifyDownload, classifyResource, isDirectDownloadLink, matchesDownloadClick, mergeResources, shouldTakeover } from './resources'

describe('resource rules', () => {
  it('filters HLS segments but retains manifests', () => {
    expect(classifyResource('https://cdn.test/a.m3u8')).toBe('hls')
    expect(classifyResource('https://cdn.test/0001.ts')).toBeNull()
  })
  it('deduplicates resources', () => {
    const item = { id: '1', url: 'https://a.test/v.mp4', kind: 'media' as const, seenAt: Date.now() }
    expect(mergeResources([item], { ...item, size: 20 })).toHaveLength(1)
    expect(mergeResources([item], { ...item, size: 20 })[0].size).toBe(20)
  })
  it('honors Alt bypass and Ctrl force', () => {
    const base = { url: 'https://a.test/file.zip', size: 20, enabled: true, minimumBytes: 10, excludedHosts: [], explicitClick: true }
    expect(shouldTakeover({ ...base, altBypass: true })).toBe(false)
    expect(shouldTakeover({ ...base, enabled: false, ctrlForce: true })).toBe(true)
  })
  it('takes over unknown-size downloads and classifies response filenames', () => {
    expect(classifyDownload('https://cdn.test/get?id=1', 'application/octet-stream', 'setup.exe')).toBe('file')
    expect(shouldTakeover({
      url: 'https://cdn.test/get?id=1', filename: 'archive.zip', size: -1,
      enabled: true, minimumBytes: 1024 * 1024, excludedHosts: [], explicitClick: true,
    })).toBe(true)
  })
  it('requires and matches a recent explicit click', () => {
    const base = { url: 'https://cdn.test/file.zip', size: 10, enabled: true, minimumBytes: 1024, excludedHosts: [] }
    expect(shouldTakeover(base)).toBe(false)
    expect(shouldTakeover({ ...base, explicitClick: true })).toBe(true)
    expect(shouldTakeover({
      ...base,
      url: 'https://cdn.test/get?id=unknown',
      filename: '',
      explicitClick: true,
    })).toBe(true)
    const intent = {
      href: 'https://cdn.test/start',
      pageUrl: 'https://site.test/download#button',
      altBypass: false,
      ctrlForce: false,
      at: 1000,
    }
    expect(matchesDownloadClick(intent, {
      url: 'https://cdn.test/final.zip',
      referrer: 'https://site.test/download',
    }, 2000)).toBe(true)
    expect(matchesDownloadClick(intent, {
      url: 'https://cdn.test/final.zip',
      referrer: 'https://other.test/',
    }, 2000)).toBe(false)
    expect(matchesDownloadClick(intent, {
      url: 'https://cdn.test/final.zip',
      referrer: 'https://site.test/download',
    }, 9000)).toBe(false)
  })
  it('only preempts links that clearly represent downloads', () => {
    expect(isDirectDownloadLink('https://cdn.test/setup.exe')).toBe(true)
    expect(isDirectDownloadLink('https://cdn.test/get?id=1', true)).toBe(true)
    expect(isDirectDownloadLink('https://site.test/article')).toBe(false)
  })
})
