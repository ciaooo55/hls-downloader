import { defineConfig } from 'wxt'
export default defineConfig({
  manifestVersion: 3,
  manifest: ({ browser }) => ({
    name: 'HLS Downloader 浏览器接管',
    description: '嗅探媒体、接管普通下载并发送到 HLS Downloader 桌面端。',
    version: '1.4.9',
    icons: {
      16: 'icon-16.png',
      32: 'icon-32.png',
      48: 'icon-48.png',
      128: 'icon-128.png',
    },
    key: browser === 'chrome' ? 'MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQDrOsVh5DPI4QgwtSbk3r66RoLAceY4j7bcvB74L8oJizTtjWwbvE31KFOR1c3qTZJUjtFgN2UDVCYThiS79RJosEDwvdeaTZPt4cwNdKINVKTcGGI8T4Pl7cqTl45IDBxUgAayjJ26YEC542os/dfVmRaZO1hDwFFhyM9AousNUwIDAQAB' : undefined,
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
        id: 'hls-downloader-store@ciaooo55.com',
        strict_min_version: '142.0',
        data_collection_permissions: { required: ['none'] },
      },
    } : undefined,
  }),
})
