// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Sruthi contributors

// Logos of the apps people dictate into, so a dictation shows where it went.
// The SVGs in src/assets/apps are from Simple Icons (CC0, https://simpleicons.org);
// the logos themselves remain trademarks of their owners and are shown only to
// identify those apps. Keys are lowercase process names without ".exe".

export interface AppInfo {
  name: string;
  /** File name (without .svg) in src/assets/apps; null when there is no logo. */
  icon: string | null;
  /** Brand colour for the logo. */
  color: string;
}

export const KNOWN_APPS: Record<string, AppInfo> = {
  "chrome": { name: "Chrome", icon: "googlechrome", color: "#4285F4" },
  "msedge": { name: "Edge", icon: "microsoftedge", color: "#0078D7" },
  "firefox": { name: "Firefox", icon: "firefox", color: "#FF7139" },
  "brave": { name: "Brave", icon: "brave", color: "#FB542B" },
  "opera": { name: "Opera", icon: "opera", color: "#FF1B2D" },
  "vivaldi": { name: "Vivaldi", icon: "vivaldi", color: "#EF3939" },
  "arc": { name: "Arc", icon: "arc", color: "#FCBFBD" },
  "code": { name: "VS Code", icon: "visualstudiocode", color: "#007ACC" },
  "cursor": { name: "Cursor", icon: "cursor", color: "#000000" },
  "windsurf": { name: "Windsurf", icon: "windsurf", color: "#0B100F" },
  "zed": { name: "Zed", icon: "zedindustries", color: "#084CCF" },
  "sublime_text": { name: "Sublime Text", icon: "sublimetext", color: "#FF9800" },
  "idea64": { name: "IntelliJ IDEA", icon: "intellijidea", color: "#000000" },
  "pycharm64": { name: "PyCharm", icon: "pycharm", color: "#000000" },
  "webstorm64": { name: "WebStorm", icon: "webstorm", color: "#000000" },
  "studio64": { name: "Android Studio", icon: "androidstudio", color: "#3DDC84" },
  "devenv": { name: "Visual Studio", icon: "visualstudio", color: "#5C2D91" },
  "windowsterminal": { name: "Terminal", icon: "windowsterminal", color: "#4D4D4D" },
  "powershell": { name: "PowerShell", icon: "powershell", color: "#5391FE" },
  "pwsh": { name: "PowerShell", icon: "powershell", color: "#5391FE" },
  "slack": { name: "Slack", icon: "slack", color: "#4A154B" },
  "discord": { name: "Discord", icon: "discord", color: "#5865F2" },
  "telegram": { name: "Telegram", icon: "telegram", color: "#26A5E4" },
  "whatsapp": { name: "WhatsApp", icon: "whatsapp", color: "#25D366" },
  "signal": { name: "Signal", icon: "signal", color: "#3B45FD" },
  "teams": { name: "Teams", icon: "microsoftteams", color: "#6264A7" },
  "ms-teams": { name: "Teams", icon: "microsoftteams", color: "#6264A7" },
  "zoom": { name: "Zoom", icon: "zoom", color: "#0B5CFF" },
  "notion": { name: "Notion", icon: "notion", color: "#000000" },
  "obsidian": { name: "Obsidian", icon: "obsidian", color: "#7C3AED" },
  "evernote": { name: "Evernote", icon: "evernote", color: "#00A82D" },
  "onenote": { name: "OneNote", icon: "microsoftonenote", color: "#7719AA" },
  "outlook": { name: "Outlook", icon: "microsoftoutlook", color: "#0078D4" },
  "winword": { name: "Word", icon: "microsoftword", color: "#2B579A" },
  "excel": { name: "Excel", icon: "microsoftexcel", color: "#217346" },
  "powerpnt": { name: "PowerPoint", icon: "microsoftpowerpoint", color: "#B7472A" },
  "figma": { name: "Figma", icon: "figma", color: "#F24E1E" },
  "claude": { name: "Claude", icon: "claude", color: "#D97757" },
  "chatgpt": { name: "ChatGPT", icon: "openai", color: "#10A37F" },
  "githubdesktop": { name: "GitHub Desktop", icon: "github", color: "#181717" },
  "warp": { name: "Warp", icon: "warp", color: "#01A4FF" },
  "notepad++": { name: "Notepad++", icon: "notepadplusplus", color: "#90E59A" },
  "postman": { name: "Postman", icon: "postman", color: "#FF6C37" },
  "docker desktop": { name: "Docker", icon: "docker", color: "#2496ED" },
  "ollama": { name: "Ollama", icon: "ollama", color: "#000000" },
  "spotify": { name: "Spotify", icon: "spotify", color: "#1ED760" },
};

const icons = import.meta.glob("./assets/apps/*.svg", {
  eager: true,
  query: "?url",
  import: "default",
}) as Record<string, string>;

/** URL of a bundled logo, or null. */
export function iconUrl(icon: string | null): string | null {
  return icon ? (icons[`./assets/apps/${icon}.svg`] ?? null) : null;
}

/** What to show for a process name such as "chrome.exe"; null when unknown. */
export function appInfo(process: string | undefined): AppInfo | null {
  const key = (process ?? "").replace(/\.exe$/i, "").trim().toLowerCase();
  if (!key || key === "unknown") return null;
  const known = KNOWN_APPS[key];
  if (known) return known;
  return { name: key.charAt(0).toUpperCase() + key.slice(1), icon: null, color: "#6b6b68" };
}
