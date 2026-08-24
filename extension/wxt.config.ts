import { defineConfig } from 'wxt'
import { CHROMIUM_PUBLIC_KEY, FIREFOX_EXTENSION_ID } from './lib/storeIdentity'
const extensionVersion = process.env.HLS_EXTENSION_VERSION || '7.0.0'

export default defineConfig({
  manifestVersion: 3,
  manifest: ({ browser }) => ({
    name: `HLS Downloader 浏览器接管 v${extensionVersion}`,
    description: '嗅探媒体、接管普通下载并发送到 HLS Downloader 桌面端。',
    version: extensionVersion,
    icons: {
      16: 'icon-16.png',
      32: 'icon-32.png',
      48: 'icon-48.png',
      128: 'icon-128.png',
    },
    key: browser === 'chrome' ? CHROMIUM_PUBLIC_KEY : undefined,
    permissions: [
      'downloads', 'contextMenus', 'nativeMessaging', 'storage', 'cookies', 'webRequest', 'alarms',
      ...(browser === 'chrome' ? ['downloads.ui', 'downloads.shelf'] : []),
      ...(browser === 'firefox' ? ['webRequestBlocking'] : []),
    ],
    host_permissions: ['<all_urls>'],
    web_accessible_resources: [{ resources: ['icon-16.png', 'icon-32.png', 'icon-48.png', 'icon-128.png'], matches: ['<all_urls>'] }],
    action: { default_title: 'HLS Downloader', default_icon: { 16: 'icon-16.png', 32: 'icon-32.png', 48: 'icon-48.png' } },
    commands: {
      'send-current-page': { suggested_key: { default: 'Ctrl+Shift+Y' }, description: '嗅探当前页面' },
    },
    browser_specific_settings: browser === 'firefox' ? {
      gecko: {
        id: FIREFOX_EXTENSION_ID,
        strict_min_version: '142.0',
        data_collection_permissions: { required: ['none'] },
      },
    } : undefined,
  }),
})
