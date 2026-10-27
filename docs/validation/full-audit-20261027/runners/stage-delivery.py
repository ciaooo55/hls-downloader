import hashlib
import json
from pathlib import Path
import shutil

root=Path(__file__).resolve().parents[4]
cache=root/'.tool-cache/full-audit-20261027'
delivery=cache/'delivery'
out=cache/'release-assets'
out.mkdir(exist_ok=True)
installed=json.loads((cache/'final-reports/installed-audit.json').read_text('utf-8-sig'))
fresh=json.loads((cache/'final-reports/fresh-installed-audit.json').read_text('utf-8-sig'))
meta=json.loads((delivery/'installer.json').read_text('utf-8-sig'))
assert hashlib.sha256((delivery/'final-candidate.msi').read_bytes()).hexdigest()==meta['sha256']
assert installed['candidate_sha256']==fresh['msi_sha256']==meta['sha256']
assert meta['release_ready'] is False, 'This staging script only delivers an explicitly incomplete candidate'
assert len(installed['checks'])==3 and len(fresh['checks'])>=8, 'Expected completed backend checks are missing'
render_path=cache/'render-reports/render-installed-audit.json'
render=json.loads(render_path.read_text('utf-8-sig')) if render_path.is_file() else None
if render:
    assert render['msi_sha256']==meta['sha256']
render_passed=bool(render and render['passed'])
mapping={
 'final-candidate.msi':'HLSDownloader-7.0.3-r19-Windows-x64-unsigned.msi',
 'HLSDownloader-7.0.3-Chromium.zip':'HLSDownloader-7.0.3-Chromium.zip',
 'HLSDownloader-7.0.3-Firefox-未签名.zip':'HLSDownloader-7.0.3-Firefox-unsigned.zip',
 'HLSDownloader-7.0.3-插件完整源码.zip':'HLSDownloader-7.0.3-extension-source.zip',
 'HLSDownloader-7.0.3-Firefox-审核源码.zip':'HLSDownloader-7.0.3-Firefox-review-source.zip',
 'BUILD-PROVENANCE.json':'BUILD-PROVENANCE.json',
}
for source,name in mapping.items():
    shutil.copyfile(delivery/source,out/name)
notes='''HLS Downloader 7.0.3-r19 未签名候选
源码快照：{commit}；UI根因修复：67f9a63。安装包SHA256：{sha}。
产品仅在Windows Sandbox安装和运行；没有在宿主安装。
本轮修复：静止任务列表无限重组导致的高CPU、任务列表POST正文丢失、跨电脑命名队列导入、导出原子覆盖，以及Core/UI/FFmpeg日志限额与脱敏。
实际验证：Core646项及显式curl/WinHTTP/FFmpeg9项；两项桌面导出/日志测试；插件418项、双构建；双浏览器接管分类验收及各13类媒体归属；HTTP/HLS/DASH下载、续传、提前播放与缓存。最终同SHA包在无虚拟GPU沙盒中的升级、回滚、任务恢复、220组件校验、自选路径、系统Downloads、真实公开视频、HTTP/HLS重试与缓存检查通过；首次界面捕获空白，整体验收为失败，finally卸载返回0。安装目录469700438字节（约448MiB）。
等待实际绘制后的独立补验：{render_status}。首次失败记录保留；后台接口就绪不能代替实际绘制或原生按钮验收。
仍未完成：Chromium所有入口从未创建下载项；首次Native Host响应曾超1500ms；用户原始直播403/HLS卡死/播放器偶发超时现场；全原生按钮、主题缩放矩阵；完整三产品性能对照（本轮IDM未开始传输）。收费字幕云端及实体投屏/TVBox不在本轮验收范围。
因此release_ready=false，不是“全部功能无问题”的正式认证。后台验证不等于实际点击验收。后续未使用宿主鼠标键盘。
Firefox插件未签名，由用户自行签名。主程序中配置字幕；插件包不含密钥。
当前记录：docs/validation/full-audit-20261027/README.md。
'''.format(commit=meta['source_commit'],sha=meta['sha256'],
           render_status='通过（包括15秒全产品空闲采样和正常卸载）' if render_passed else '未通过或未完成，详见验收记录')
(out/'VALIDATION.txt').write_text(notes,'utf8')
manifest={
 'verified_production_source_commit':meta['source_commit'], 'unsigned':True,
 'release_ready':False, 'package_tier':'candidate','host_install_performed':False,
 'github_release_tag':'v7.0.3-r19','validation_report':'docs/validation/full-audit-20261027/README.md',
 'validation':{'core_tests_passed':646,'explicit_ignored_cases_passed':9,
   'default_core_ignored':12,'remaining_default_ignored_not_run':3,
   'extension_tests_passed':418,'browser_hover_cases':{'Firefox':13,'Edge':13},
   'strict_chromium_zero_download_item_all_entries':False,
   'native_host_first_response_threshold':False,
   'final_initial_combined_audit_passed':installed['passed'],
   'final_no_gpu_upgrade_rollback_checkpoint':True,'final_installed_payload':True,
   'final_initial_ui_capture':fresh.get('ui_capture'),
   'final_render_followup_passed':render_passed,
   'installed_bytes':fresh['installed_bytes'],
   'full_native_button_matrix':False,'three_product_current_comparison':False},
 'known_limits':notes.split('仍未完成：',1)[1].split('\n',1)[0],
 'assets':[]}
for path in sorted(out.iterdir()):
    if path.name in ('DELIVERY-MANIFEST.json','SHA256SUMS.txt') or not path.is_file():continue
    manifest['assets'].append({'name':path.name,'bytes':path.stat().st_size,
       'sha256':hashlib.sha256(path.read_bytes()).hexdigest()})
(out/'DELIVERY-MANIFEST.json').write_text(json.dumps(manifest,ensure_ascii=False,indent=2)+'\n','utf8')
(out/'SHA256SUMS.txt').write_text(''.join(hashlib.sha256(p.read_bytes()).hexdigest()+'  '+p.name+'\n'
  for p in sorted(out.iterdir()) if p.is_file() and p.name!='SHA256SUMS.txt'),'utf8')
print(json.dumps({'source_commit':meta['source_commit'],'msi_sha256':meta['sha256'],'assets':len(list(out.iterdir()))}))
