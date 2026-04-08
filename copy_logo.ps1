$source = Join-Path $env:USERPROFILE ".gemini\antigravity\brain\$env:CONVERSATION_ID\media__1774881864798.png"
if(Test-Path $source) {
    New-Item -ItemType Directory -Force -Path ".\assets"
    Copy-Item -Path $source -Destination ".\assets\logo.png" -Force
} else {
    Write-Host "File not found"
}
