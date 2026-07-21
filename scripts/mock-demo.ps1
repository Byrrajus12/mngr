$ErrorActionPreference = "Stop"

function Send-MngrEvent {
  param(
    [Parameter(Mandatory = $true)]
    [hashtable] $Payload,

    [Parameter(Mandatory = $true)]
    [string] $Label
  )

  $json = $Payload | ConvertTo-Json -Depth 20 -Compress
  $lastError = $null

  for ($attempt = 1; $attempt -le 40; $attempt++) {
    $client = $null
    $writer = $null

    try {
      $client = [System.IO.Pipes.NamedPipeClientStream]::new(".", "mngr", [System.IO.Pipes.PipeDirection]::Out)
      $client.Connect(250)

      $writer = [System.IO.StreamWriter]::new($client, [System.Text.UTF8Encoding]::new($false))
      $writer.NewLine = "`n"
      $writer.WriteLine($json)
      $writer.Flush()
      Write-Host ("[{0:HH:mm:ss}] {1}" -f (Get-Date), $Label)
      return
    } catch {
      $lastError = $_
      Start-Sleep -Milliseconds 100
    } finally {
      if ($writer) { $writer.Dispose() }
      if ($client) { $client.Dispose() }
    }
  }

  throw "Could not connect to \\.\pipe\mngr after retries: $lastError"
}

function New-Event {
  param(
    [Parameter(Mandatory = $true)] [string] $SessionId,
    [Parameter(Mandatory = $true)] [string] $EventName,
    [Parameter(Mandatory = $true)] [string] $Cwd,
    [string] $ToolName,
    [hashtable] $ToolInput,
    [hashtable] $ToolResponse,
    [string] $Prompt,
    [string] $Message
  )

  $payload = @{
    session_id = $SessionId
    hook_event_name = $EventName
    cwd = $Cwd
  }

  if ($ToolName) { $payload.tool_name = $ToolName }
  if ($ToolInput) { $payload.tool_input = $ToolInput }
  if ($ToolResponse) { $payload.tool_response = $ToolResponse }
  if ($Prompt) { $payload.prompt = $Prompt }
  if ($Message) { $payload.message = $Message }

  return $payload
}

function Send-Step {
  param(
    [Parameter(Mandatory = $true)] [hashtable] $Payload,
    [Parameter(Mandatory = $true)] [string] $Label,
    [int] $PauseMs = 900
  )

  Send-MngrEvent -Payload $Payload -Label $Label
  if ($PauseMs -gt 0) { Start-Sleep -Milliseconds $PauseMs }
}

$runId = [guid]::NewGuid().ToString("N").Substring(0, 8)
$s1 = "demo-api-server-$runId"
$s2 = "demo-web-ui-$runId"
$s3 = "demo-mngr-$runId"

$base = Split-Path -Parent (Get-Location).Path
$api = Join-Path $base "api-server"
$web = Join-Path $base "web-ui"
$mngr = (Get-Location).Path

Write-Host "Starting mngr multi-session demo ($runId). Keep the overlay visible while this runs."

Send-Step (New-Event $s1 "SessionStart" $api) "api-server: session started" 700
Send-Step (New-Event $s1 "UserPromptSubmit" $api -Prompt "Add request tracing and verify the API") "api-server: prompt submitted" 700
Send-Step (New-Event $s1 "PreToolUse" $api -ToolName "Bash" -ToolInput @{ command = "cargo test tracing"; description = "Run tracing tests" }) "api-server: running cargo test" 1300
Send-Step (New-Event $s1 "PostToolUse" $api -ToolName "Bash" -ToolInput @{ command = "cargo test tracing" } -ToolResponse @{ success = $true; exit_code = 0; duration_ms = 1240 }) "api-server: tests passed" 500
Send-Step (New-Event $s1 "PreToolUse" $api -ToolName "Edit" -ToolInput @{ file_path = "src/tracing.rs"; old_string = "trace_id"; new_string = "request_id" }) "api-server: editing tracing.rs" 1200

Send-Step (New-Event $s2 "SessionStart" $web) "web-ui: session started while api-server is working" 600
Send-Step (New-Event $s2 "UserPromptSubmit" $web -Prompt "Polish the dashboard cards") "web-ui: prompt submitted" 600
Send-Step (New-Event $s2 "PreToolUse" $web -ToolName "Bash" -ToolInput @{ command = "npm run lint"; description = "Lint dashboard" }) "web-ui: running npm run lint" 1100

Send-Step (New-Event $s1 "PermissionRequest" $api -ToolName "Bash" -ToolInput @{ command = "cargo install sqlx-cli"; description = "Install migration helper" }) "api-server: permission requested for Bash" 1800

Send-Step (New-Event $s3 "SessionStart" $mngr) "mngr: session started" 600
Send-Step (New-Event $s3 "UserPromptSubmit" $mngr -Prompt "Refine the overlay rail states") "mngr: prompt submitted" 600
Send-Step (New-Event $s3 "PreToolUse" $mngr -ToolName "Edit" -ToolInput @{ file_path = "src/components/Filament.tsx"; old_string = "idle"; new_string = "demo state" }) "mngr: editing filament" 1300

Send-Step (New-Event $s2 "PostToolUse" $web -ToolName "Bash" -ToolInput @{ command = "npm run lint" } -ToolResponse @{ success = $true; exit_code = 0; duration_ms = 1010 }) "web-ui: lint passed" 500
Send-Step (New-Event $s2 "PreToolUse" $web -ToolName "Edit" -ToolInput @{ file_path = "src/Dashboard.tsx"; old_string = "Card"; new_string = "MetricCard" }) "web-ui: editing dashboard" 1000
Send-Step (New-Event $s2 "PostToolUse" $web -ToolName "Edit" -ToolInput @{ file_path = "src/Dashboard.tsx" } -ToolResponse @{ success = $true; lines_changed = 18 }) "web-ui: edit complete" 500
Send-Step (New-Event $s2 "Stop" $web) "web-ui: stopped (done card/segment)" 1800

Send-Step (New-Event $s1 "PreToolUse" $api -ToolName "Bash" -ToolInput @{ command = "cargo install sqlx-cli"; description = "Permission resolved, resuming" }) "api-server: permission resolved, resuming work" 1000
Send-Step (New-Event $s1 "PostToolUse" $api -ToolName "Bash" -ToolInput @{ command = "cargo install sqlx-cli" } -ToolResponse @{ success = $true; exit_code = 0; duration_ms = 980 }) "api-server: install command finished" 700
Send-Step (New-Event $s1 "Stop" $api) "api-server: stopped (done card/segment)" 1400

Send-Step (New-Event $s3 "PostToolUse" $mngr -ToolName "Edit" -ToolInput @{ file_path = "src/components/Filament.tsx" } -ToolResponse @{ success = $true; lines_changed = 24 }) "mngr: filament edit complete" 900
Send-Step (New-Event $s3 "PreToolUse" $mngr -ToolName "Bash" -ToolInput @{ command = "npm run build"; description = "Verify overlay" }) "mngr: running npm run build" 1900
Send-Step (New-Event $s3 "PostToolUse" $mngr -ToolName "Bash" -ToolInput @{ command = "npm run build" } -ToolResponse @{ success = $true; exit_code = 0; duration_ms = 1640 }) "mngr: build passed" 700
Send-Step (New-Event $s3 "Stop" $mngr) "mngr: stopped (all sessions done)" 0

Write-Host "Demo complete. Done cards stay visible until you click to dismiss them."