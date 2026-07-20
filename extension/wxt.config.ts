import { defineConfig } from 'wxt'
export default defineConfig({
  modules: ['@wxt-dev/module-react'],
  manifestVersion: 3,
  manifest: ({ browser }) => ({
    name: 'HLS Downloader 浏览器接管',
    description: '嗅探媒体、接管普通下载并发送到 HLS Downloader 桌面端。',
    version: '1.2.3',
    icons: {
      512: 'icon.png',
    },
    key: browser === 'chrome' ? 'MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQDrOsVh5DPI4QgwtSbk3r66RoLAceY4j7bcvB74L8oJizTtjWwbvE31KFOR1c3qTZJUjtFgN2UDVCYThiS79RJosEDwvdeaTZPt4cwNdKINVKTcGGI8T4Pl7cqTl45IDBxUgAayjJ26YEC542os/dfVmRaZO1hDwFFhyM9AousNUwIDAQAB' : undefined,
    permissions: ['downloads', 'contextMenus', 'nativeMessaging', 'storage', 'cookies', 'webRequest'],
    host_permissions: ['<all_urls>'],
    action: { default_title: 'HLS Downloader' },
    commands: {
      'send-current-page': { suggested_key: { default: 'Ctrl+Shift+Y' }, description: '嗅探当前页面' },
    },
    browser_specific_settings: browser === 'firefox' ? {
      gecko: {
        id: 'browser@hls-downloader.ciaooo55.com',
        strict_min_version: '142.0',
        data_collection_permissions: { required: ['none'] },
      },
    } : undefined,
  }),
})
