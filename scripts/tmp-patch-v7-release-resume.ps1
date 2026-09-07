$ErrorActionPreference = 'Stop'

$path = '.github/workflows/release-v7.yml'
$source = (Get-Content -LiteralPath $path -Raw -Encoding UTF8) -replace "`r`n", "`n"

$oldPreflight = @'
          $existingTag = ((git ls-remote --tags origin "refs/tags/$tag") -join '').Trim()
          if ($existingTag) { throw "Release tag already exists: $tag" }
          if ([String]::IsNullOrWhiteSpace($env:HLS_V7_SIGN_CERT_THUMBPRINT)) { throw 'HLS_V7_SIGN_CERT_THUMBPRINT secret is required for a formal release.' }
'@
$newPreflight = @'
          $existingTag = ((git ls-remote --tags origin "refs/tags/$tag") -join '').Trim()
          $releaseMode = 'fresh'
          if ($existingTag) {
            git fetch --force origin "refs/tags/$tag:refs/tags/$tag"
            if ($LASTEXITCODE -ne 0) { throw "Failed to fetch existing release tag: $tag" }
            $tagType = (git cat-file -t "refs/tags/$tag").Trim()
            if ($tagType -ne 'tag') { throw "Existing release tag is not annotated: $tag" }
            $tagCommit = (git rev-parse "refs/tags/$tag^{}").Trim()
            if ($tagCommit -ne $head) { throw "Existing release tag points at another commit: tag=$tagCommit HEAD=$head" }

            $headers = @{
              Authorization = "Bearer $env:GH_TOKEN"
              Accept = 'application/vnd.github+json'
              'X-GitHub-Api-Version' = '2022-11-28'
              'User-Agent' = 'hls-downloader-release-workflow'
            }
            $release = $null
            try {
              $release = Invoke-RestMethod -Method Get -Uri "https://api.github.com/repos/$env:GITHUB_REPOSITORY/releases/tags/$tag" -Headers $headers
            } catch {
              $status = 0
              if ($_.Exception.Response -and $_.Exception.Response.StatusCode) { $status = [int]$_.Exception.Response.StatusCode }
              if ($status -ne 404) { throw }
            }
            if ($release) {
              if (-not [bool]$release.draft) { throw "Release already exists and is not a draft: $tag" }
              $releaseMode = 'draft'
            } else {
              $releaseMode = 'tag-only'
            }
          }
          "HLS_V7_RELEASE_MODE=$releaseMode" >> $env:GITHUB_ENV
          if ([String]::IsNullOrWhiteSpace($env:HLS_V7_SIGN_CERT_THUMBPRINT)) { throw 'HLS_V7_SIGN_CERT_THUMBPRINT secret is required for a formal release.' }
'@
$oldPreflight = (($oldPreflight -replace "`r`n", "`n").TrimEnd())
$newPreflight = (($newPreflight -replace "`r`n", "`n").TrimEnd())
if (-not $source.Contains($oldPreflight)) { throw 'Formal release preflight block no longer matches the expected source.' }
$source = $source.Replace($oldPreflight, $newPreflight)

$oldCreate = @'
          git config user.name 'github-actions[bot]'
          git config user.email '41898282+github-actions[bot]@users.noreply.github.com'
          git tag -a $env:HLS_V7_TAG -m "HLS Downloader $env:HLS_V7_VERSION" $head
          git push origin $env:HLS_V7_TAG
          $stage = (Resolve-Path 'artifacts\v7-productization\release-staging').Path
          $assets = @(Get-ChildItem -LiteralPath $stage -File | Select-Object -ExpandProperty FullName)
          $notes = Join-Path $stage "README-$env:HLS_V7_VERSION.txt"
          gh release create $env:HLS_V7_TAG @assets --repo $env:GITHUB_REPOSITORY --title "HLS Downloader $env:HLS_V7_VERSION" --notes-file $notes --draft --verify-tag
          if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
'@
$newCreate = @'
          git config user.name 'github-actions[bot]'
          git config user.email '41898282+github-actions[bot]@users.noreply.github.com'
          if ($env:HLS_V7_RELEASE_MODE -eq 'fresh') {
            git tag -a $env:HLS_V7_TAG -m "HLS Downloader $env:HLS_V7_VERSION" $head
            git push origin $env:HLS_V7_TAG
            if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
          } elseif ($env:HLS_V7_RELEASE_MODE -notin @('tag-only', 'draft')) {
            throw "Unknown release resume mode: $env:HLS_V7_RELEASE_MODE"
          }

          $stage = (Resolve-Path 'artifacts\v7-productization\release-staging').Path
          $assetFiles = @(Get-ChildItem -LiteralPath $stage -File)
          $assets = @($assetFiles | Select-Object -ExpandProperty FullName)
          $expectedNames = @($assetFiles | Select-Object -ExpandProperty Name)
          $notes = Join-Path $stage "README-$env:HLS_V7_VERSION.txt"

          if ($env:HLS_V7_RELEASE_MODE -eq 'draft') {
            $release = gh api "repos/$env:GITHUB_REPOSITORY/releases/tags/$env:HLS_V7_TAG" | ConvertFrom-Json
            if (-not $release.draft) { throw 'Existing release stopped being a draft before resume.' }
            foreach ($existingAsset in @($release.assets)) {
              if ($existingAsset.name -notin $expectedNames) { throw "Draft release contains an unexpected asset: $($existingAsset.name)" }
            }
            gh release edit $env:HLS_V7_TAG --repo $env:GITHUB_REPOSITORY --title "HLS Downloader $env:HLS_V7_VERSION" --notes-file $notes --draft
            if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
            gh release upload $env:HLS_V7_TAG @assets --repo $env:GITHUB_REPOSITORY --clobber
            if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
          } else {
            gh release create $env:HLS_V7_TAG @assets --repo $env:GITHUB_REPOSITORY --title "HLS Downloader $env:HLS_V7_VERSION" --notes-file $notes --draft --verify-tag
            if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
          }
'@
$oldCreate = (($oldCreate -replace "`r`n", "`n").TrimEnd())
$newCreate = (($newCreate -replace "`r`n", "`n").TrimEnd())
if (-not $source.Contains($oldCreate)) { throw 'Formal release tag/draft block no longer matches the expected source.' }
$source = $source.Replace($oldCreate, $newCreate)
Set-Content -LiteralPath $path -Value $source -Encoding utf8NoBOM -NoNewline

$doc = 'docs/v7-release-runner.md'
$docText = Get-Content -LiteralPath $doc -Raw -Encoding UTF8
if ($docText -notmatch 'Safe retry semantics') {
  $docText += @'

## Safe retry semantics

The formal workflow is resumable only for release state that can be proven to belong to the same frozen `main` commit:

- no tag: create a fresh annotated tag and draft release;
- annotated tag on the exact current commit, but no release: reuse the validated tag and create the draft;
- annotated tag on the exact current commit with an existing draft release: reject unexpected assets, refresh title/notes, and replace only the expected staged assets before digest verification.

A lightweight tag, a tag pointing at any other commit, or an already-published release is always rejected. This lets an interrupted draft upload be retried without weakening the source, signing, gate, or digest checks.
'@
  Set-Content -LiteralPath $doc -Value $docText -Encoding utf8NoBOM -NoNewline
}
