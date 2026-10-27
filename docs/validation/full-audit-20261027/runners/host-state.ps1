$ErrorActionPreference='Stop'
$result=[ordered]@{checked_at=[DateTime]::UtcNow.ToString('o');host_install_performed=$false;registry_writes_performed=$false;scope='Exact product keys, related MSI products, product process names and named default directories; no whole-disk zero-trace claim';native_host_keys=@();uninstall_records=@();related_msi_products=@();product_processes=@();named_directories=@()}
$parents=@('Google\Chrome','Microsoft\Edge','BraveSoftware\Brave-Browser','Chromium','Vivaldi','Opera Software','Mozilla')
foreach($hive in @([Microsoft.Win32.RegistryHive]::CurrentUser,[Microsoft.Win32.RegistryHive]::LocalMachine)){
    foreach($view in @([Microsoft.Win32.RegistryView]::Registry32,[Microsoft.Win32.RegistryView]::Registry64)){
        $base=[Microsoft.Win32.RegistryKey]::OpenBaseKey($hive,$view)
        try {
            foreach($parent in $parents){
                $name="Software\$parent\NativeMessagingHosts\com.ciaooo55.hls_downloader"
                $key=$base.OpenSubKey($name)
                if($key){try{$result.native_host_keys+=@{hive=[string]$hive;view=[string]$view;key=$name;value=$key.GetValue('')}}finally{$key.Dispose()}}
            }
            $uninstall=$base.OpenSubKey('Software\Microsoft\Windows\CurrentVersion\Uninstall')
            if($uninstall){
                try {
                    foreach($name in $uninstall.GetSubKeyNames()){
                        $key=$uninstall.OpenSubKey($name)
                        if($key){
                            try {
                                if([string]$key.GetValue('DisplayName') -match '^HLS\s?Downloader(?:\s|$)'){
                                    $result.uninstall_records+=@{hive=[string]$hive;view=[string]$view;key=$name;display_name=$key.GetValue('DisplayName');version=$key.GetValue('DisplayVersion')}
                                }
                            } finally {$key.Dispose()}
                        }
                    }
                } finally {$uninstall.Dispose()}
            }
        } finally {$base.Dispose()}
    }
}
$installer=New-Object -ComObject WindowsInstaller.Installer
try{$result.related_msi_products=@($installer.RelatedProducts('{1C80D5F7-A1EC-4BAE-A4A6-E010C5A3EE6B}'))}finally{[Runtime.InteropServices.Marshal]::FinalReleaseComObject($installer)|Out-Null}
$names=@('HLSDownloader.exe','HLSDownloaderEngine.exe','HLSDownloaderNativeHost.exe','HLSDownloaderPresenter.exe','HLSDownloaderUpdater.exe')
$result.product_processes=@(Get-CimInstance Win32_Process | Where-Object {$_.Name -in $names} | Select-Object Name,ProcessId,ExecutablePath)
foreach($path in @((Join-Path $env:LOCALAPPDATA 'HLSDownloader'),(Join-Path $env:APPDATA 'HLSDownloader'),(Join-Path $env:LOCALAPPDATA 'Programs\HLSDownloader'))){if(Test-Path -LiteralPath $path){$result.named_directories+=$path}}
$out=Join-Path $PSScriptRoot '..\results\host-state.json'
$result | ConvertTo-Json -Depth 7 | Set-Content -LiteralPath $out -Encoding UTF8
$result | ConvertTo-Json -Depth 7
