<#
.SYNOPSIS
  Try Deck on deck-server (the server-only Herdr build) without touching your Herdr.

.DESCRIPTION
  1. Builds deck-server (release) from this repo's server/ crate. Rebuilds
     when the sources are newer than the exe, or with -Rebuild.
  2. Starts `deck-server --session try` in the background (no window) unless it runs,
     and waits until its pipe answers `ping`.
  3. Starts a standalone debug build of this repo's Deck (or the installed Deck
     with -Installed)
     pointed at that pipe, with its own data dir and window-state file.

  Stop the trial server with:  scripts\try-deck-server.ps1 -Stop

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts\try-deck-server.ps1
#>
[CmdletBinding()]
param(
    [switch]$Rebuild,
    [switch]$Installed,
    [switch]$Stop,
    [switch]$NoDeck,   # build and start the server only
    [string]$Session = 'try'
)

$ErrorActionPreference = 'Stop'
$DeckRepo = Split-Path $PSScriptRoot -Parent
$ServerDir = Join-Path $DeckRepo 'server'
$Exe = Join-Path $ServerDir 'target\release\deck-server.exe'
$SessionDir = Join-Path $env:APPDATA "deck-server\sessions\$Session"
$Socket = Join-Path $SessionDir 'herdr.sock'
$DeckData = Join-Path $env:LOCALAPPDATA 'netrunner-deck-try'

function Say($text) { Write-Host $text -ForegroundColor Cyan }

function Get-TrialServer {
    Get-Process deck-server -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and ($_.Path -ieq $Exe) }
}

# One JSON line to the server's named pipe; the reply line or $null.
function Invoke-Ping {
    $pipe = New-Object System.IO.Pipes.NamedPipeClientStream('.', $Socket, [System.IO.Pipes.PipeDirection]::InOut)
    try {
        $pipe.Connect(1000)
        $writer = New-Object System.IO.StreamWriter($pipe)
        $writer.AutoFlush = $true
        $writer.WriteLine('{"id":"try","method":"ping","params":{}}')
        $reader = New-Object System.IO.StreamReader($pipe)
        return $reader.ReadLine()
    } catch {
        return $null
    } finally {
        $pipe.Dispose()
    }
}

if ($Stop) {
    if (-not (Test-Path $Exe)) { Say 'deck-server is not built.'; exit 0 }
    & $Exe stop --session $Session
    Say "Its workspaces stay saved in $SessionDir."
    exit 0
}

# ---- 1. build ---------------------------------------------------------------------
$newestSource = Get-ChildItem (Join-Path $ServerDir 'src'), (Join-Path $ServerDir 'crates') -Recurse -File -Include *.rs, *.zig, *.toml |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
$stale = -not (Test-Path $Exe) -or ((Get-Item $Exe).LastWriteTime -lt $newestSource.LastWriteTime)
if ($Rebuild -or $stale) {
    if (Get-TrialServer) { throw 'deck-server is running; stop it first: scripts\try-deck-server.ps1 -Stop' }
    if (-not $env:ZIG) {
        $zig = Join-Path $env:USERPROFILE '.local\zig-0.16.0\zig.exe'
        if (-not (Test-Path $zig)) { throw "Zig 0.16.0 not found at $zig (set `$env:ZIG)." }
        $env:ZIG = $zig
    }
    Say 'Building deck-server (release). The first build takes a few minutes...'
    Push-Location $ServerDir
    try { cargo build --release --locked --bin deck-server } finally { Pop-Location }
    if ($LASTEXITCODE) { throw 'cargo build failed' }
}

# ---- 2. server --------------------------------------------------------------------
if (Get-TrialServer) {
    Say 'deck-server is already running.'
} else {
    # deck-server starts itself detached with no window (as herdr does) and
    # returns once its API answers.
    Say "Starting deck-server --session $Session in the background..."
    & $Exe --session $Session
    if ($LASTEXITCODE) { throw 'deck-server did not start' }
}
$pong = $null
foreach ($i in 1..40) {
    $pong = Invoke-Ping
    if ($pong -and $pong -match '"pong"') { break }
    Start-Sleep -Milliseconds 250
}
if (-not ($pong -match '"pong"')) { throw "deck-server did not answer on $Socket" }
Say "deck-server answers on \\.\pipe\$Socket"

# ---- 3. Deck ----------------------------------------------------------------------
# Deck reads the server from these; set here, they reach only the Deck started below.
$env:HERDR_SOCKET_PATH = $Socket
$env:HERDR_CONFIG_DIR = $SessionDir   # archive reads deck-server's session.json, not Herdr's
$env:DECK_DATA_DIR = $DeckData        # own archive/quickhacks/uplink and window-state file

Write-Host @"

What you should see
  * Deck opens ONLINE with deck-server connected and an empty tree:
    deck-server keeps its own sessions (and your Herdr keeps running).
  * Your Herdr is found but not connected: click the NETRUNNER DECK tab
    (or the gear next to the clock) and + CONNECT it to see both servers,
    one sidebar section each. Alt+0 shows all, Alt+1/2 one server.
  * Create a workspace or tab with + and type in it like in Herdr.
  * The terminal view refreshes by polling (this build has no pane.output_changed
    yet), so output may lag a fraction of a second.
  * This Deck keeps its archive and settings in $DeckData.

When done: close Deck, then run
  powershell -ExecutionPolicy Bypass -File scripts\try-deck-server.ps1 -Stop

"@

if ($NoDeck) { exit 0 }

if ($Installed) {
    $deckExe = @(
        (Join-Path $env:LOCALAPPDATA 'Netrunner Deck\netrunner-deck.exe'),
        (Join-Path $env:ProgramFiles 'Netrunner Deck\netrunner-deck.exe')
    ) | Where-Object { Test-Path $_ } | Select-Object -First 1
    if (-not $deckExe) { throw 'installed Deck not found; run without -Installed' }
    Write-Warning 'The installed Deck (v0.3.0) predates DECK_DATA_DIR: it shares archive and window state with your normal Deck, and its archive lists your Herdr sessions.'
    Start-Process -FilePath $deckExe
} else {
    # A standalone debug build of this repo in its own target dir: no vite dev
    # server (port 1420) and no lock on target\debug\netrunner-deck.exe, so a
    # `npm run tauri dev` you already run is not disturbed.
    $env:CARGO_TARGET_DIR = Join-Path $DeckRepo 'src-tauri\target\try'
    $deckExe = Join-Path $env:CARGO_TARGET_DIR 'debug\netrunner-deck.exe'
    # Its own app identifier: its own WebView2 profile, log dir and window state.
    # Sharing the normal Deck's WebView2 profile makes both windows share one
    # browser process (and its keyboard focus and localStorage).
    $tryConf = Join-Path $PSScriptRoot 'tauri.try.conf.json'
    $newestDeck = Get-ChildItem (Join-Path $DeckRepo 'src'), (Join-Path $DeckRepo 'src-tauri\src'), $tryConf -Recurse -File |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if ($Rebuild -or -not (Test-Path $deckExe) -or ((Get-Item $deckExe).LastWriteTime -lt $newestDeck.LastWriteTime)) {
        if (Get-Process netrunner-deck -ErrorAction SilentlyContinue | Where-Object { $_.Path -ieq $deckExe }) {
            throw 'the trial Deck is running; close it first'
        }
        Say 'Building Deck from this repo (debug, no installer). The first build takes a few minutes...'
        Push-Location $DeckRepo
        try { npx tauri build --debug --no-bundle --config $tryConf } finally { Pop-Location }
        if ($LASTEXITCODE) { throw 'Deck build failed' }
    }
    Say "Starting Deck ($deckExe)..."
    Start-Process -FilePath $deckExe
}
