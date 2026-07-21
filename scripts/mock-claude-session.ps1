$ErrorActionPreference = "Stop"

function Send-MngrEvent {
  param(
    [Parameter(Mandatory = $true)]
    [hashtable] $Payload
  )

  $json = $Payload | ConvertTo-Json -Depth 20 -Compress
  $lastError = $null

  for ($attempt = 1; $attempt -le 20; $attempt++) {
    $client = $null
    $writer = $null

    try {
      $client = [System.IO.Pipes.NamedPipeClientStream]::new(".", "mngr", [System.IO.Pipes.PipeDirection]::Out)
      $client.Connect(250)

      $writer = [System.IO.StreamWriter]::new($client, [System.Text.UTF8Encoding]::new($false))
      $writer.NewLine = "`n"
      $writer.WriteLine($json)
      $writer.Flush()
      return
    } catch {
      $lastError = $_
      Start-Sleep -Milliseconds 75
    } finally {
      if ($writer) { $writer.Dispose() }
      if ($client) { $client.Dispose() }
    }
  }

  throw "Could not connect to \\.\pipe\mngr after retries: $lastError"
}

$sessionId = "mock-session-" + [guid]::NewGuid().ToString()
$cwd = (Get-Location).Path

$events = @(
  @{
    session_id = $sessionId
    hook_event_name = "SessionStart"
    cwd = $cwd
  },
  @{
    session_id = $sessionId
    hook_event_name = "UserPromptSubmit"
    cwd = $cwd
    prompt = "Add a session manager and overlay UI"
  },
  @{
    session_id = $sessionId
    hook_event_name = "PreToolUse"
    cwd = $cwd
    tool_name = "Bash"
    tool_input = @{ command = "npm run build"; description = "Verify the frontend" }
  },
  @{
    session_id = $sessionId
    hook_event_name = "PostToolUse"
    cwd = $cwd
    tool_name = "Bash"
    tool_input = @{ command = "npm run build"; description = "Verify the frontend" }
    tool_response = @{ success = $true; exit_code = 0; duration_ms = 1260 }
  },
  @{
    session_id = $sessionId
    hook_event_name = "PreToolUse"
    cwd = $cwd
    tool_name = "Edit"
    tool_input = @{ file_path = "src/App.tsx"; old_string = "raw event log"; new_string = "session overlay" }
  },
  @{
    session_id = $sessionId
    hook_event_name = "PostToolUse"
    cwd = $cwd
    tool_name = "Edit"
    tool_input = @{ file_path = "src/App.tsx"; old_string = "raw event log"; new_string = "session overlay" }
    tool_response = @{ success = $true; lines_changed = 42 }
  },
  @{
    session_id = $sessionId
    hook_event_name = "Stop"
    cwd = $cwd
  }
)

foreach ($event in $events) {
  Send-MngrEvent -Payload $event
  Start-Sleep -Milliseconds 350
}

Write-Host "Sent $($events.Count) mock events for $sessionId"