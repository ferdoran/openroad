// Custom YARA rules targeted at VSRO-leak threats.
//
// Coverage:
//   - Crypto miners (xmrig, monero pools)
//   - Discord/Telegram exfil channels
//   - C2-style URLs (suspicious TLDs, hardcoded IPs)
//   - Packer signatures (UPX, ASPack, common obfuscators)
//   - Suspicious Windows API combinations (process injection, autorun)
//   - Embedded PE / overlay anomalies
//   - Tor onion addresses

rule SRO_Miner_XMRig
{
    meta:
        description = "XMRig cryptocurrency miner indicators"
        severity    = "high"
    strings:
        $a = "xmrig"  ascii nocase
        $b = "stratum+tcp://" ascii nocase
        $c = "monero" ascii nocase wide
        $d = "randomx" ascii nocase
        $e = "donate-level" ascii nocase
    condition:
        2 of them
}

rule SRO_Miner_Pool
{
    meta:
        description = "Known mining-pool host strings"
        severity    = "high"
    strings:
        $a = "minexmr.com" ascii nocase
        $b = "supportxmr.com" ascii nocase
        $c = "nanopool.org" ascii nocase
        $d = "moneroocean.stream" ascii nocase
        $e = "pool.minexmr" ascii nocase
        $f = "f2pool.com" ascii nocase
    condition:
        any of them
}

rule SRO_Discord_Webhook
{
    meta:
        description = "Discord webhook URL — common exfil channel"
        severity    = "high"
    strings:
        $a = /discord(app)?\.com\/api\/webhooks\/[0-9]{16,}\/[A-Za-z0-9_-]{20,}/
    condition:
        any of them
}

rule SRO_Telegram_Bot
{
    meta:
        description = "Telegram bot token / API endpoint — exfil channel"
        severity    = "high"
    strings:
        $a = /api\.telegram\.org\/bot[0-9]{8,}:[A-Za-z0-9_-]{20,}/
        $b = "sendMessage" ascii
        $c = "sendDocument" ascii
    condition:
        $a and 1 of ($b, $c)
}

rule SRO_Tor_Onion
{
    meta:
        description = "Tor .onion v3 address (56 chars)"
        severity    = "medium"
    strings:
        $a = /[a-z2-7]{56}\.onion/
    condition:
        any of them
}

rule SRO_Suspicious_TLD
{
    meta:
        description = "URL on a typo-squat / abuse-prone TLD"
        severity    = "low"
    strings:
        $a = /https?:\/\/[a-z0-9.-]{3,40}\.(top|xyz|tk|ml|ga|cf|gq|pw|click|cyou|monster|rest|hair)\b/
    condition:
        any of them
}

rule SRO_Packer_UPX
{
    meta:
        description = "UPX-packed binary"
        severity    = "medium"
    strings:
        $a = "UPX0" ascii
        $b = "UPX1" ascii
        $c = "UPX!" ascii
    condition:
        2 of them
}

rule SRO_Packer_ASPack
{
    meta:
        description = "ASPack-packed binary"
        severity    = "medium"
    strings:
        $a = ".aspack" ascii
        $b = "aPLib" ascii
    condition:
        any of them
}

rule SRO_Process_Injection_Combo
{
    meta:
        description = "Imports used together by process-injection malware"
        severity    = "medium"
    strings:
        $a = "VirtualAllocEx" ascii
        $b = "WriteProcessMemory" ascii
        $c = "CreateRemoteThread" ascii
        $d = "NtUnmapViewOfSection" ascii
        $e = "QueueUserAPC" ascii
    condition:
        3 of them
}

rule SRO_Persistence_Autorun
{
    meta:
        description = "Registry autorun-key writes"
        severity    = "medium"
    strings:
        $a = "Software\\Microsoft\\Windows\\CurrentVersion\\Run" ascii nocase
        $b = "Software\\Microsoft\\Windows\\CurrentVersion\\RunOnce" ascii nocase
        $c = "RegSetValueExA" ascii
        $d = "RegCreateKeyExA" ascii
    condition:
        any of ($a, $b) and any of ($c, $d)
}

rule SRO_Keylogger_Indicators
{
    meta:
        description = "Keylogger API combo"
        severity    = "medium"
    strings:
        $a = "SetWindowsHookEx" ascii
        $b = "GetAsyncKeyState" ascii
        $c = "GetKeyboardState" ascii
        $d = "MapVirtualKey" ascii
    condition:
        2 of them
}

rule SRO_AntiAnalysis_Combo
{
    meta:
        description = "VM/debugger evasion API combo"
        severity    = "medium"
    strings:
        $a = "IsDebuggerPresent" ascii
        $b = "CheckRemoteDebuggerPresent" ascii
        $c = "NtQueryInformationProcess" ascii
        $d = "VBoxHook" ascii nocase
        $e = "VMware" ascii nocase
        $f = "vbox" ascii nocase
    condition:
        2 of them
}

rule SRO_PowerShell_Download_Cradle
{
    meta:
        description = "PowerShell download-and-execute one-liner pattern"
        severity    = "high"
    strings:
        $a = "powershell" ascii nocase
        $b = "DownloadString" ascii
        $c = "DownloadFile" ascii
        $d = "IEX" ascii
        $e = "Invoke-Expression" ascii nocase
        $f = "-EncodedCommand" ascii nocase
        $g = "-nop -w hidden" ascii nocase
    condition:
        $a and 1 of ($b, $c, $d, $e, $f, $g)
}

rule SRO_Bitcoin_Address
{
    meta:
        description = "Bitcoin address (P2PKH/P2SH/Bech32) embedded — ransom note or donate"
        severity    = "low"
    strings:
        $p2pkh   = /\b[13][a-km-zA-HJ-NP-Z1-9]{25,34}\b/
        $bech32  = /\bbc1[ac-hj-np-z02-9]{11,71}\b/
    condition:
        any of them
}

rule SRO_Suspicious_Shell_Strings
{
    meta:
        description = "cmd.exe / shell command-execution strings"
        severity    = "low"
    strings:
        $a = "cmd.exe /c " ascii nocase
        $b = "cmd /c " ascii nocase
        $c = "WScript.Shell" ascii
        $d = "Shell.Application" ascii
        $e = "schtasks /create" ascii nocase
        $f = "vssadmin delete shadows" ascii nocase
    condition:
        any of them
}

rule SRO_Embedded_PE_Anomaly
{
    meta:
        description = "MZ header at non-zero offset → embedded/dropped PE"
        severity    = "medium"
    strings:
        $mz = { 4D 5A }     // MZ
        $pe = { 50 45 00 00 } // PE\0\0
    condition:
        #mz > 1 and #pe > 1
}
