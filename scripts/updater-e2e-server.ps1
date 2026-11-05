#requires -Version 7.2
<#
.SYNOPSIS
  M5 更新器端到端测试的本地 release 服务器（windows-plan r4 §2.4 / §3 / §6 M5；M-7、S-R6、C-3）。

.DESCRIPTION
  三个模式：

  -Replay   由真实 GitHub release API 响应（src-tauri/tests/fixtures/release-*.json）改写出本次用例的
            release.json，并把要提供的安装包复制到服务目录（S-R6）。改写规则（§6 M5「只替换 tag、资产名、
            URL、digest、size，immutable 设为 true，其余字段原样保留」）：
              - tag_name ← -Tag；name / html_url / tarball_url / zipball_url 中的旧 tag 字面量 ← -Tag；
              - 夹具必须恰有一个资产，改写其 name、browser_download_url、digest、size
                （content_type、state、label、id、uploader 等原样保留）；
              - immutable ← true（-Case ImmutableFalse 时为 false，C-3）；
              - 其余字段（含 draft、prerelease、body、时间戳）一律不动。-SelfTest 会逐字段比对，确认改动只落在上述字段。
            -Case：
              Happy          digest / size 与实际文件一致；
              WrongDigest    digest 改为另一个合法但不匹配的 sha256；
              WrongSize      size 改为实际字节数 + 1；
              ImmutableFalse immutable 设为 false（C-3：客户端必须拒绝一键更新）。
            「PE 版本与 tag 不符」「降级」不改 JSON 字段，由调用方用 -Tag 与 -AssetFile 的组合构造
            （例如 -Tag v0.3.2 配 0.3.1 / 0.3.0 的安装包），见 scripts/updater-e2e-cases.ps1。

  -Serve    用 System.Net.HttpListener 只在 http://127.0.0.1:<port>/ 上提供 -Root 下的静态文件：
              - 只接受 GET / HEAD，其余 405；
              - 路径各段必须匹配 [A-Za-z0-9._-]+ 且不是 . / ..，否则 400（不做任何解码后的路径拼接）；
              - 只提供已存在的普通文件，否则 404；.json 为 application/json，其余 application/octet-stream；
              - 每个请求以一行 JSON 追加到 -RequestLog（供用例断言客户端请求了什么）；
              - 就绪后把端口写入 -ReadyFile；最长运行 -MaxSeconds 秒后自动退出。
            -Port 0（默认）时自动选一个空闲端口。

  -SelfTest 不依赖 Windows：用夹具跑全部改写用例并逐字段比对，再起一个 -Serve 子进程验证请求处理。

  客户端如何指向本服务器（环境变量名、下载 URL 的路径形状）属于 M5 更新器核心的接口（Pit）。
  这里默认按 GitHub 的形状生成 browser_download_url：<BaseUrl><DownloadPath>/<tag>/<name>，
  DownloadPath 默认 /JTee77/clash-assist/releases/download，待核心定稿后再对齐。

.EXAMPLE
  pwsh scripts/updater-e2e-server.ps1 -Replay -Fixture src-tauri/tests/fixtures/release-clash-assist-v0.2.9.json `
       -Tag v0.3.1 -AssetFile dist/Clash.Assist_0.3.1_x64-setup.exe -BaseUrl http://127.0.0.1:18080 -OutDir $env:RUNNER_TEMP/e2e-root
  pwsh scripts/updater-e2e-server.ps1 -Serve -Root $env:RUNNER_TEMP/e2e-root -ReadyFile ready.txt -RequestLog requests.jsonl
  pwsh scripts/updater-e2e-server.ps1 -SelfTest

  退出码：0 = 成功；1 = 失败；2 = 用法错误。
#>
[CmdletBinding(DefaultParameterSetName = 'Serve')]
param(
  [Parameter(Mandatory, ParameterSetName = 'Replay')][switch]$Replay,
  [Parameter(Mandatory, ParameterSetName = 'Replay')][string]$Fixture,
  [Parameter(Mandatory, ParameterSetName = 'Replay')][string]$Tag,
  [Parameter(Mandatory, ParameterSetName = 'Replay')][string]$AssetFile,
  [Parameter(ParameterSetName = 'Replay')][string]$AssetName,
  [Parameter(Mandatory, ParameterSetName = 'Replay')][string]$BaseUrl,
  [Parameter(ParameterSetName = 'Replay')][string]$DownloadPath = '/JTee77/clash-assist/releases/download',
  [Parameter(ParameterSetName = 'Replay')][ValidateSet('Happy', 'WrongDigest', 'WrongSize', 'ImmutableFalse')][string]$Case = 'Happy',
  [Parameter(Mandatory, ParameterSetName = 'Replay')][string]$OutDir,

  [Parameter(ParameterSetName = 'Serve')][switch]$Serve,
  [Parameter(Mandatory, ParameterSetName = 'Serve')][string]$Root,
  [Parameter(ParameterSetName = 'Serve')][int]$Port = 0,
  [Parameter(ParameterSetName = 'Serve')][string]$ReadyFile,
  [Parameter(ParameterSetName = 'Serve')][string]$RequestLog,
  [Parameter(ParameterSetName = 'Serve')][int]$MaxSeconds = 1800,

  [Parameter(Mandatory, ParameterSetName = 'SelfTest')][switch]$SelfTest
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$SegmentRe = '^[A-Za-z0-9._-]+$'
$TagRe = '^v\d+\.\d+\.\d+$'
$Utf8NoBom = [Text.UTF8Encoding]::new($false)

function Get-Sha256Hex([string]$path) { (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash.ToLowerInvariant() }

function ConvertTo-JsonText([Text.Json.Nodes.JsonNode]$node) {
  $opt = [Text.Json.JsonSerializerOptions]::new()
  $opt.WriteIndented = $true
  $opt.Encoder = [Text.Encodings.Web.JavaScriptEncoder]::UnsafeRelaxedJsonEscaping
  $node.ToJsonString($opt)
}

function Get-StringProp([Text.Json.Nodes.JsonObject]$o, [string]$key) {
  $n = $null
  if (-not $o.TryGetPropertyValue($key, [ref]$n) -or $null -eq $n) { return $null }
  if ($n.GetValueKind() -ne [Text.Json.JsonValueKind]::String) { return $null }
  $n.GetValue[string]()
}

# 返回 @{ Json = <string>; AssetRelPath = <string>; Sha256 = <hex>; Size = <long> }，不写文件（便于自测）
function New-ReplayRelease([string]$fixtureText, [string]$tag, [string]$assetFile, [string]$assetName,
  [string]$baseUrl, [string]$downloadPath, [string]$case) {
  if ($tag -notmatch $TagRe) { throw "-Tag 必须形如 vX.Y.Z：$tag" }
  $ver = $tag.Substring(1)
  if (-not $assetName) { $assetName = "Clash.Assist_${ver}_x64-setup.exe" }
  if ($assetName -notmatch $SegmentRe) { throw "资产名含非法字符：$assetName" }
  $u = [Uri]$baseUrl
  if ($u.Scheme -ne 'http' -or $u.Host -ne '127.0.0.1' -or $u.AbsolutePath -ne '/' -or $u.Query -or $u.Fragment) {
    throw "-BaseUrl 只能是 http://127.0.0.1:<port>：$baseUrl"
  }
  $segs = @($downloadPath.Trim('/') -split '/' | Where-Object { $_ -ne '' })
  foreach ($s in $segs) { if ($s -notmatch $SegmentRe -or $s -in '.', '..') { throw "-DownloadPath 段非法：$s" } }

  $root = [Text.Json.Nodes.JsonNode]::Parse($fixtureText)
  if ($root -isnot [Text.Json.Nodes.JsonObject]) { throw '夹具不是 JSON 对象' }
  $oldTag = Get-StringProp $root 'tag_name'
  if (-not $oldTag) { throw '夹具缺少字符串 tag_name' }
  $assets = $root['assets']
  if ($assets -isnot [Text.Json.Nodes.JsonArray] -or $assets.Count -ne 1) { throw '夹具必须恰有一个资产（真实响应原样保留，只改写这一项）' }
  $asset = $assets[0]

  $sha = Get-Sha256Hex $assetFile
  $size = (Get-Item -LiteralPath $assetFile).Length

  $root['tag_name'] = [Text.Json.Nodes.JsonValue]::Create($tag)
  foreach ($k in 'name', 'html_url', 'tarball_url', 'zipball_url') {
    $v = Get-StringProp $root $k
    if ($null -ne $v) { $root[$k] = [Text.Json.Nodes.JsonValue]::Create($v.Replace($oldTag, $tag)) }
  }
  $rel = (@($segs) + @($tag, $assetName)) -join '/'
  $digest = "sha256:$sha"
  if ($case -eq 'WrongDigest') {
    # 另一个合法的 64 位十六进制：翻转最后一位（保证与实际不同）
    $last = $sha[-1]; $flip = if ($last -eq '0') { '1' } else { '0' }
    $digest = 'sha256:' + $sha.Substring(0, 63) + $flip
  }
  $declSize = if ($case -eq 'WrongSize') { $size + 1 } else { $size }
  $asset['name'] = [Text.Json.Nodes.JsonValue]::Create($assetName)
  $asset['browser_download_url'] = [Text.Json.Nodes.JsonValue]::Create($baseUrl.TrimEnd('/') + '/' + $rel)
  $asset['digest'] = [Text.Json.Nodes.JsonValue]::Create($digest)
  $asset['size'] = [Text.Json.Nodes.JsonValue]::Create([long]$declSize)
  $root['immutable'] = [Text.Json.Nodes.JsonValue]::Create([bool]($case -ne 'ImmutableFalse'))
  @{ Json = (ConvertTo-JsonText $root); AssetRelPath = $rel; Sha256 = $sha; Size = $size; AssetName = $assetName }
}

function Invoke-Replay {
  $text = [IO.File]::ReadAllText((Resolve-Path -LiteralPath $Fixture))
  $r = New-ReplayRelease $text $Tag (Resolve-Path -LiteralPath $AssetFile).Path $AssetName $BaseUrl $DownloadPath $Case
  if (Test-Path -LiteralPath $OutDir) { throw "-OutDir 已存在（每个用例用新目录，避免混入上一个用例的文件）：$OutDir" }
  New-Item -ItemType Directory -Path $OutDir | Out-Null
  [IO.File]::WriteAllText((Join-Path $OutDir 'release.json'), $r.Json, $Utf8NoBom)
  $dst = Join-Path $OutDir ($r.AssetRelPath -replace '/', [IO.Path]::DirectorySeparatorChar)
  New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dst) | Out-Null
  Copy-Item -LiteralPath $AssetFile -Destination $dst
  Write-Host "replay：case=$Case tag=$Tag 资产=$($r.AssetName) sha256=$($r.Sha256) size=$($r.Size) → $OutDir"
}

function Get-FreePort {
  $l = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
  $l.Start(); $p = ([Net.IPEndPoint]$l.LocalEndpoint).Port; $l.Stop(); $p
}

function Resolve-RequestPath([string]$rootFull, [string]$rawPath) {
  # 只看原始路径（不解码）：任何 % 编码、反斜杠、空段、. / .. 都拒绝
  if (-not $rawPath.StartsWith('/')) { return @{ Status = 400 } }
  $p = $rawPath.Split('?')[0]
  $segs = $p.Substring(1).Split('/')
  if ($segs.Count -eq 0 -or ($segs.Count -eq 1 -and $segs[0] -eq '')) { return @{ Status = 404 } }
  foreach ($s in $segs) { if ($s -notmatch $SegmentRe -or $s -in '.', '..') { return @{ Status = 400 } } }
  $full = [IO.Path]::GetFullPath((Join-Path $rootFull ($segs -join [IO.Path]::DirectorySeparatorChar)))
  if (-not $full.StartsWith($rootFull + [IO.Path]::DirectorySeparatorChar, [StringComparison]::Ordinal)) { return @{ Status = 400 } }
  if (-not (Test-Path -LiteralPath $full -PathType Leaf)) { return @{ Status = 404 } }
  @{ Status = 200; Path = $full }
}

function Invoke-Serve {
  $rootFull = [IO.Path]::GetFullPath((Resolve-Path -LiteralPath $Root).Path).TrimEnd([IO.Path]::DirectorySeparatorChar)
  $listener = $null
  for ($i = 0; $i -lt 5 -and -not $listener; $i++) {
    $p = if ($Port -gt 0) { $Port } else { Get-FreePort }
    $l = [Net.HttpListener]::new()
    $l.Prefixes.Add("http://127.0.0.1:$p/")
    try { $l.Start(); $listener = $l; $script:BoundPort = $p }
    catch { $l.Close(); if ($Port -gt 0) { throw } }
  }
  if (-not $listener) { throw '无法启动 HttpListener' }
  Write-Host "updater-e2e-server：http://127.0.0.1:$script:BoundPort/ → $rootFull（最长 $MaxSeconds 秒）"
  if ($ReadyFile) { [IO.File]::WriteAllText($ReadyFile, "$script:BoundPort", $Utf8NoBom) }
  $deadline = [DateTime]::UtcNow.AddSeconds($MaxSeconds)
  try {
    while ([DateTime]::UtcNow -lt $deadline) {
      $task = $listener.GetContextAsync()
      while (-not $task.Wait(500)) { if ([DateTime]::UtcNow -ge $deadline) { break } }
      if (-not $task.IsCompleted) { break }
      $ctx = $task.Result
      $req = $ctx.Request; $res = $ctx.Response
      $raw = $req.RawUrl
      $status = 0; $bytes = 0
      try {
        if ($req.HttpMethod -notin 'GET', 'HEAD') { $status = 405; $res.StatusCode = 405; $res.ContentLength64 = 0 }
        else {
          $r = Resolve-RequestPath $rootFull $raw
          $status = $r.Status
          $res.StatusCode = $status
          if ($status -ne 200) { $res.ContentLength64 = 0 }
          else {
            $res.ContentType = if ($r.Path.EndsWith('.json')) { 'application/json; charset=utf-8' } else { 'application/octet-stream' }
            $len = (Get-Item -LiteralPath $r.Path).Length
            $res.ContentLength64 = $len
            if ($req.HttpMethod -eq 'GET') {
              $fs = [IO.File]::OpenRead($r.Path)
              try { $fs.CopyTo($res.OutputStream); $bytes = $len } finally { $fs.Dispose() }
            }
          }
        }
      } catch {
        $status = -1
        Write-Host "::warning::处理 $raw 时出错：$($_.Exception.Message)"
      } finally {
        try { $res.Close() } catch { }
      }
      if ($RequestLog) {
        $line = [ordered]@{ t = [DateTime]::UtcNow.ToString('o'); method = $req.HttpMethod; path = $raw; status = $status; bytes = $bytes; ua = $req.UserAgent } | ConvertTo-Json -Compress
        [IO.File]::AppendAllText($RequestLog, $line + "`n", $Utf8NoBom)
      }
    }
  } finally {
    $listener.Stop(); $listener.Close()
  }
}

# ---------- 自测 ----------
function Get-JsonDiffPaths($a, $b, [string]$path, [Collections.Generic.List[string]]$out) {
  if ($null -eq $a -or $null -eq $b) { if (-not ($null -eq $a -and $null -eq $b)) { $out.Add($path) }; return }
  if ($a -is [Text.Json.Nodes.JsonObject]) {
    if ($b -isnot [Text.Json.Nodes.JsonObject]) { $out.Add($path); return }
    $keys = [Collections.Generic.HashSet[string]]::new()
    foreach ($kv in $a) { [void]$keys.Add($kv.Key) }
    foreach ($kv in $b) { [void]$keys.Add($kv.Key) }
    foreach ($k in $keys) {
      $x = $null; $y = $null
      $ha = $a.TryGetPropertyValue($k, [ref]$x); $hb = $b.TryGetPropertyValue($k, [ref]$y)
      if ($ha -ne $hb) { $out.Add("$path.$k"); continue }
      Get-JsonDiffPaths $x $y "$path.$k" $out
    }
    return
  }
  if ($a -is [Text.Json.Nodes.JsonArray]) {
    if ($b -isnot [Text.Json.Nodes.JsonArray] -or $a.Count -ne $b.Count) { $out.Add($path); return }
    for ($i = 0; $i -lt $a.Count; $i++) { Get-JsonDiffPaths $a[$i] $b[$i] "$path[$i]" $out }
    return
  }
  if ($a.ToJsonString() -ne $b.ToJsonString()) { $out.Add($path) }
}

function Invoke-SelfTest {
  $fail = 0
  function Check([bool]$cond, [string]$msg) { if ($cond) { Write-Host "ok   $msg" } else { Write-Host "FAIL $msg"; $script:stFail = 1 } }
  $script:stFail = 0
  $repo = Split-Path -Parent $PSScriptRoot
  $fixtures = @(Get-ChildItem -LiteralPath (Join-Path $repo 'src-tauri/tests/fixtures') -Filter 'release-clash-assist-*.json' -File)
  Check ($fixtures.Count -ge 1) "找到真实 release 夹具（$($fixtures.Count) 个）"
  $tmp = Join-Path ([IO.Path]::GetTempPath()) ("ca-e2e-selftest-" + [Guid]::NewGuid().ToString('N'))
  New-Item -ItemType Directory -Path $tmp | Out-Null
  $serverProc = $null
  try {
    $asset = Join-Path $tmp 'fake-setup.exe'
    $payload = [byte[]]::new(70000); [Random]::new(7).NextBytes($payload)
    [IO.File]::WriteAllBytes($asset, $payload)
    $sha = Get-Sha256Hex $asset
    $allowed = @('$.tag_name', '$.name', '$.html_url', '$.tarball_url', '$.zipball_url', '$.immutable',
      '$.assets[0].name', '$.assets[0].browser_download_url', '$.assets[0].digest', '$.assets[0].size')
    foreach ($fx in $fixtures) {
      $text = [IO.File]::ReadAllText($fx.FullName)
      $orig = [Text.Json.Nodes.JsonNode]::Parse($text)
      $oldTag = $orig['tag_name'].GetValue[string]()
      foreach ($case in 'Happy', 'WrongDigest', 'WrongSize', 'ImmutableFalse') {
        $r = New-ReplayRelease $text 'v0.3.1' $asset $null 'http://127.0.0.1:18080' '/JTee77/clash-assist/releases/download' $case
        $j = [Text.Json.Nodes.JsonNode]::Parse($r.Json)
        $diff = [Collections.Generic.List[string]]::new()
        Get-JsonDiffPaths $orig $j '$' $diff
        $extra = @($diff | Where-Object { $_ -notin $allowed })
        Check ($extra.Count -eq 0) "$($fx.Name) $case：改动只落在允许的字段（多出：$($extra -join ', ')）"
        $a = $j['assets'][0]
        Check ($j['tag_name'].GetValue[string]() -eq 'v0.3.1') "$case：tag_name = v0.3.1"
        $stale = @('name', 'html_url', 'tarball_url', 'zipball_url' | Where-Object { $null -ne $j[$_] -and $j[$_].ToJsonString().Contains($oldTag) })
        Check ($stale.Count -eq 0) "$case：name / html_url / tarball_url / zipball_url 中不再含旧 tag $oldTag（残留：$($stale -join ', ')）"
        Check ($a['name'].GetValue[string]() -eq 'Clash.Assist_0.3.1_x64-setup.exe') "$case：资产名符合 V8"
        Check ($a['browser_download_url'].GetValue[string]() -eq 'http://127.0.0.1:18080/JTee77/clash-assist/releases/download/v0.3.1/Clash.Assist_0.3.1_x64-setup.exe') "$case：browser_download_url"
        Check ($j['draft'].ToJsonString() -eq $orig['draft'].ToJsonString() -and $j['prerelease'].ToJsonString() -eq $orig['prerelease'].ToJsonString()) "$case：draft / prerelease 原样保留"
        $wantImm = $case -ne 'ImmutableFalse'
        Check ($j['immutable'].GetValueKind() -eq $(if ($wantImm) { [Text.Json.JsonValueKind]::True } else { [Text.Json.JsonValueKind]::False })) "$case：immutable = $wantImm（布尔）"
        $d = $a['digest'].GetValue[string]()
        Check ($d -match '^sha256:[0-9a-f]{64}$') "$case：digest 形如 sha256:<64 hex>"
        Check (($d -eq "sha256:$sha") -eq ($case -ne 'WrongDigest')) "$case：digest $(if ($case -eq 'WrongDigest') { '与文件不符' } else { '与文件一致' })"
        $s = $a['size'].GetValue[long]()
        Check (($s -eq $payload.Length) -eq ($case -ne 'WrongSize')) "$case：size $(if ($case -eq 'WrongSize') { '与文件不符' } else { '与文件一致' })"
      }
    }
    # 参数校验
    $bad = @(
      @{ N = 'tag 不是 vX.Y.Z'; T = '0.3.1'; B = 'http://127.0.0.1:1' }
      @{ N = 'BaseUrl 不是 127.0.0.1'; T = 'v0.3.1'; B = 'http://localhost:1' }
      @{ N = 'BaseUrl 是 https'; T = 'v0.3.1'; B = 'https://127.0.0.1:1' }
      @{ N = 'BaseUrl 带路径'; T = 'v0.3.1'; B = 'http://127.0.0.1:1/x' }
    )
    foreach ($b in $bad) {
      $threw = $false
      try { New-ReplayRelease ([IO.File]::ReadAllText($fixtures[0].FullName)) $b.T $asset $null $b.B '/a' 'Happy' | Out-Null } catch { $threw = $true }
      Check $threw "拒绝：$($b.N)"
    }

    # Serve：起子进程，验证请求处理
    $root = Join-Path $tmp 'root'
    $r = New-ReplayRelease ([IO.File]::ReadAllText($fixtures[0].FullName)) 'v0.3.1' $asset $null 'http://127.0.0.1:18080' '/JTee77/clash-assist/releases/download' 'Happy'
    New-Item -ItemType Directory -Path $root | Out-Null
    [IO.File]::WriteAllText((Join-Path $root 'release.json'), $r.Json, $Utf8NoBom)
    $dst = Join-Path $root ($r.AssetRelPath -replace '/', [IO.Path]::DirectorySeparatorChar)
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dst) | Out-Null
    Copy-Item -LiteralPath $asset -Destination $dst
    [IO.File]::WriteAllText((Join-Path $tmp 'secret.txt'), 'outside-root', $Utf8NoBom)
    $ready = Join-Path $tmp 'ready.txt'; $log = Join-Path $tmp 'requests.jsonl'
    $pwsh = (Get-Process -Id $PID).Path
    $serverProc = Start-Process -FilePath $pwsh -PassThru -NoNewWindow -ArgumentList @(
      '-NoProfile', '-File', $PSCommandPath, '-Serve', '-Root', $root, '-ReadyFile', $ready, '-RequestLog', $log, '-MaxSeconds', '120')
    $t0 = [DateTime]::UtcNow
    while (-not (Test-Path -LiteralPath $ready) -and ([DateTime]::UtcNow - $t0).TotalSeconds -lt 30) { Start-Sleep -Milliseconds 200 }
    Check (Test-Path -LiteralPath $ready) 'Serve 子进程就绪'
    $port = [int](Get-Content -Raw -LiteralPath $ready)
    $base = "http://127.0.0.1:$port"
    $http = [Net.Http.HttpClient]::new()
    function Get-Status([string]$rawPath, [string]$method = 'GET') {
      # 用 TcpClient 发原始请求行，确保 ..、%2e 等不被客户端规范化
      $c = [Net.Sockets.TcpClient]::new('127.0.0.1', $port)
      try {
        $s = $c.GetStream()
        $reqText = "$method $rawPath HTTP/1.1`r`nHost: 127.0.0.1:$port`r`nContent-Length: 0`r`nConnection: close`r`n`r`n"
        $b = [Text.Encoding]::ASCII.GetBytes($reqText); $s.Write($b, 0, $b.Length)
        $sr = [IO.StreamReader]::new($s, [Text.Encoding]::ASCII)
        $first = $sr.ReadLine()
        [int]($first -split ' ')[1]
      } finally { $c.Dispose() }
    }
    $body = $http.GetStringAsync("$base/release.json").GetAwaiter().GetResult()
    Check ($body -eq $r.Json) 'GET /release.json 返回改写后的 JSON'
    $bin = $http.GetByteArrayAsync("$base/$($r.AssetRelPath)").GetAwaiter().GetResult()
    $hex = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($bin)).ToLowerInvariant()
    Check ($hex -eq $sha -and $bin.Length -eq $payload.Length) 'GET 安装包：字节与 sha256 一致'
    Check ((Get-Status "/$($r.AssetRelPath)" 'HEAD') -eq 200) 'HEAD 安装包 200'
    Check ((Get-Status '/nope.json') -eq 404) '不存在的文件 404'
    Check ((Get-Status '/') -eq 404) '根路径 404'
    Check ((Get-Status '/../secret.txt') -eq 400) '.. 段 400'
    Check ((Get-Status '/%2e%2e/secret.txt') -eq 400) '%2e%2e 400（不解码）'
    Check ((Get-Status '/JTee77%2fclash-assist') -eq 400) '%2f 400'
    Check ((Get-Status '/release.json' 'POST') -eq 405) 'POST 405'
    $http.Dispose()
    $lines = @(Get-Content -LiteralPath $log | ForEach-Object { $_ | ConvertFrom-Json })
    Check ($lines.Count -ge 9) "请求日志逐条记录（$($lines.Count) 条）"
    Check (@($lines | Where-Object { $_.path -eq '/release.json' -and $_.status -eq 200 }).Count -eq 1) '请求日志含 /release.json 200'
  } finally {
    if ($serverProc -and -not $serverProc.HasExited) { Stop-Process -Id $serverProc.Id -Force -ErrorAction SilentlyContinue }
    Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
  }
  Write-Host ("updater-e2e-server 自测：" + $(if ($script:stFail) { '失败' } else { '全部通过' }))
  exit $script:stFail
}

try {
  switch ($PSCmdlet.ParameterSetName) {
    'Replay' { Invoke-Replay; exit 0 }
    'Serve' { Invoke-Serve; exit 0 }
    'SelfTest' { Invoke-SelfTest }
  }
} catch {
  Write-Host "::error::updater-e2e-server：$($_.Exception.Message)"
  exit 1
}
