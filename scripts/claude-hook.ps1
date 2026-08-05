$ErrorActionPreference = "SilentlyContinue"
$Utf8NoBom = [System.Text.UTF8Encoding]::new($false, $true)
[Console]::InputEncoding = $Utf8NoBom
[Console]::OutputEncoding = $Utf8NoBom


function ConvertTo-MngrHookPayload {
  param([string]$PayloadText)

  $hook = $PayloadText | ConvertFrom-Json
  if ($hook.hook_event_name -eq "PermissionRequest") {
    $requestId = [guid]::NewGuid().ToString()
    $hook | Add-Member -NotePropertyName "request_id" -NotePropertyValue $requestId -Force
  }
  $hook | Add-Member -NotePropertyName "wt_session" -NotePropertyValue ([string]$env:WT_SESSION) -Force
  $hook | Add-Member -NotePropertyName "hook_pid" -NotePropertyValue $PID -Force

  $knownHosts = @('WindowsTerminal.exe', 'Code.exe', 'conhost.exe', 'OpenConsole.exe', 'ConEmuC64.exe', 'ConEmu.exe', 'Hyper.exe', 'Alacritty.exe', 'wezterm-gui.exe', 'cmd.exe')
  $walkPid = $PID
  $shellPid = $null
  $hookParentPid = $null
  while ($walkPid -and $walkPid -gt 0) {
    $proc = Get-CimInstance Win32_Process -Filter "ProcessId=$walkPid" -ErrorAction SilentlyContinue
    if (-not $proc) { break }
    if ($walkPid -eq $PID) {
      $hookParentPid = $proc.ParentProcessId
    }
    $parentProc = Get-CimInstance Win32_Process -Filter "ProcessId=$($proc.ParentProcessId)" -ErrorAction SilentlyContinue
    if ($parentProc -and $knownHosts -contains $parentProc.Name) {
      $shellPid = $walkPid
      break
    }
    $walkPid = $proc.ParentProcessId
  }
  if ($null -eq $shellPid) {
    $walkPid2 = $PID
    while ($walkPid2 -and $walkPid2 -gt 0) {
      $proc2 = Get-CimInstance Win32_Process -Filter "ProcessId=$walkPid2" -ErrorAction SilentlyContinue
      if (-not $proc2) { break }
      if ($proc2.Name -eq 'claude.exe') {
        $shellPid = $proc2.ParentProcessId
        break
      }
      $walkPid2 = $proc2.ParentProcessId
    }
  }
  if ($null -eq $shellPid) {
    $shellPid = $hookParentPid
  }
  $hook | Add-Member -NotePropertyName "shell_pid" -NotePropertyValue $shellPid -Force

  return $hook
}

function Read-MngrApprovalResponse {
  param([string]$Path)

  for ($i = 0; $i -lt 10; $i++) {
    try {
      if (Test-Path -LiteralPath $Path) {
        $content = Get-Content -Raw -Encoding UTF8 -LiteralPath $Path -ErrorAction Stop
        if (-not [string]::IsNullOrWhiteSpace($content)) {
          return ($content | ConvertFrom-Json -ErrorAction Stop)
        }
      }
    } catch {
      Start-Sleep -Milliseconds 50
    }
  }

  return $null
}

function Read-MngrUtf8Stdin {
  $inputStream = [Console]::OpenStandardInput()
  $memory = [System.IO.MemoryStream]::new()
  $inputStream.CopyTo($memory)
  return $Utf8NoBom.GetString($memory.ToArray())
}

function ConvertTo-ClaudePermissionOutput {
  param(
    [string]$Decision,
    [string]$Reason,
    $UpdatedPermissions
  )

  $validDecision = if (@("allow", "deny") -contains $Decision) { $Decision } else { "deny" }
  $decisionObject = @{
    behavior = $validDecision
  }

  if ($validDecision -eq "deny") {
    $decisionObject.message = if ($null -eq $Reason) { "" } else { $Reason }
    $decisionObject.interrupt = $false
  } elseif ($null -ne $UpdatedPermissions) {
    $decisionObject.updatedPermissions = $UpdatedPermissions
  }

  @{
    "continue" = $true
    suppressOutput = $true
    hookSpecificOutput = @{
      hookEventName = "PermissionRequest"
      decision = $decisionObject
    }
  } | ConvertTo-Json -Compress -Depth 10
}

function Test-MngrQuestionResponse {
  param($Response)

  if ($null -eq $Response) { return $false }
  $names = $Response.PSObject.Properties.Name
  return ($names -contains "updatedInput") -or ($names -contains "answers") -or ($names -contains "answer")
}

function ConvertTo-ClaudeQuestionOutput {
  param($Response)

  $updatedInput = $Response.updatedInput
  if ($null -eq $updatedInput) {
    $updatedInput = @{
      questions = $Response.questions
      answers = $Response.answers
      answer = $Response.answer
    }
  }

  @{
    "continue" = $true
    suppressOutput = $true
    hookSpecificOutput = @{
      hookEventName = "PermissionRequest"
      decision = @{
        behavior = "allow"
        updatedInput = $updatedInput
      }
    }
  } | ConvertTo-Json -Compress -Depth 100
}

$payload = Read-MngrUtf8Stdin
if ([string]::IsNullOrWhiteSpace($payload)) {
  exit 0
}

try {
  $mngrPayload = ConvertTo-MngrHookPayload -PayloadText $payload.Trim().TrimStart([char]0xFEFF)
} catch {
  exit 0
}

if ($mngrPayload.hook_event_name -eq "PermissionRequest") {
  $mode = $mngrPayload.permission_mode
  if ($mode -eq "bypassPermissions" -or $mode -eq "acceptEdits") {
    exit 0
  }
}

if ($env:MNGR_HOOK_TEST_MODE -eq "payload") {
  $mngrPayload | ConvertTo-Json -Compress -Depth 100
  exit 0
}

if ($env:MNGR_HOOK_TEST_MODE -eq "permission-output") {
  $decision = if ([string]::IsNullOrWhiteSpace($env:MNGR_HOOK_TEST_DECISION)) { "allow" } else { $env:MNGR_HOOK_TEST_DECISION }
  $reason = if ([string]::IsNullOrWhiteSpace($env:MNGR_HOOK_TEST_REASON)) { "" } else { $env:MNGR_HOOK_TEST_REASON }
  $updatedPermissions = if ($decision -eq "allow") { $mngrPayload.permission_suggestions } else { $null }
  ConvertTo-ClaudePermissionOutput -Decision $decision -Reason $reason -UpdatedPermissions $updatedPermissions
  exit 0
}

$jsonLine = ($mngrPayload | ConvertTo-Json -Compress -Depth 100)

$pipeWriteSucceeded = $false
try {
  $client = [System.IO.Pipes.NamedPipeClientStream]::new(".", "mngr", [System.IO.Pipes.PipeDirection]::Out)
  $client.Connect(50)

  $pipeBytes = $Utf8NoBom.GetBytes($jsonLine + "`n")
  $client.Write($pipeBytes, 0, $pipeBytes.Length)
  $client.Flush()
  $pipeWriteSucceeded = $true
} catch {
} finally {
  if ($null -ne $client) {
    $client.Dispose()
  }
}

if (($mngrPayload.hook_event_name -ne "PermissionRequest") -or (-not $pipeWriteSucceeded)) {
  exit 0
}

$responsesRoot = Join-Path $env:LOCALAPPDATA "mngr\responses"
$responsePath = Join-Path $responsesRoot "$($mngrPayload.request_id).json"
$deadline = (Get-Date).AddSeconds(3600)

while ((Get-Date) -lt $deadline) {
  $response = Read-MngrApprovalResponse -Path $responsePath
  if ($null -ne $response) {
    Remove-Item -LiteralPath $responsePath -Force -ErrorAction SilentlyContinue
    if (Test-MngrQuestionResponse -Response $response) {
      ConvertTo-ClaudeQuestionOutput -Response $response
    } else {
      ConvertTo-ClaudePermissionOutput -Decision ([string]$response.decision) -Reason ([string]$response.reason) -UpdatedPermissions $response.updatedPermissions
    }
    exit 0
  }

  Start-Sleep -Milliseconds 200
}

exit 0
