import { describe, expect, it } from 'vitest'
import { compareNumericVersions, extensionNeedsUpgrade } from './version'

describe('extension version update checks', () => {
  it('compares numeric browser extension versions', () => {
    expect(compareNumericVersions('3.0.8', '3.0.8')).toBe(0)
    expect(compareNumericVersions('3.0.7', '3.0.8')).toBe(-1)
    expect(compareNumericVersions('3.1', '3.0.99')).toBe(1)
  })

  it('orders the clean-line major upgrade numerically', () => {
    expect(compareNumericVersions('10.0.0', '7.0.10')).toBe(1)
    expect(compareNumericVersions('7.0.10', '10.0.0')).toBe(-1)
    expect(extensionNeedsUpgrade('7.0.10', '10.0.0')).toBe(true)
    expect(extensionNeedsUpgrade('10.0.0', '7.0.10')).toBe(false)
    expect(compareNumericVersions('10.1.0', '10.0.0')).toBe(1)
    expect(compareNumericVersions('10.0.0', '10.1.0')).toBe(-1)
    expect(extensionNeedsUpgrade('10.0.0', '10.1.0')).toBe(true)
    expect(extensionNeedsUpgrade('10.1.0', '10.0.0')).toBe(false)
  })

  it('only prompts when the desktop recommends a newer extension', () => {
    expect(extensionNeedsUpgrade('3.0.7', '3.0.8')).toBe(true)
    expect(extensionNeedsUpgrade('3.0.8', '3.0.8')).toBe(false)
    expect(extensionNeedsUpgrade('3.0.8', '')).toBe(false)
  })
})
