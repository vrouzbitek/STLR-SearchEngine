export type ActionKind =
  | "search"
  | "spotify"
  | "folder"
  | "app"
  | "file"
  | "youtube"
  | "google"
  | "instagram"
  | "url"
  | "calc"
  | "system"
  | "bookmark"
  | "tab"
  | "action"
  | "recent";

export const BADGE: Record<ActionKind, string> = {
  search: "Search",
  spotify: "Spotify",
  folder: "Folder",
  app: "App",
  file: "File",
  youtube: "YouTube",
  google: "Google",
  instagram: "Instagram",
  url: "Open URL",
  calc: "Calculator",
  system: "System",
  bookmark: "Bookmark",
  tab: "Tab",
  action: "Action",
  recent: "Recent",
};

const svg = {
  width: 24,
  height: 24,
  viewBox: "0 0 24 24",
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 1.8,
  strokeLinecap: "round" as const,
  strokeLinejoin: "round" as const,
};

// Phase 2: pass `src` (base64 data URL or URL of an icon extracted from the exe/file) to override the built-in glyph.
export default function ActionIcon({ kind, src }: { kind: ActionKind; src?: string }) {
  if (src) return <img src={src} width={24} height={24} className="shrink-0" alt="" />;
  switch (kind) {
    case "spotify":
      return (
        <svg {...svg} fill="#1DB954" stroke="none" className="shrink-0">
          <path d="M12 0a12 12 0 1 0 0 24 12 12 0 0 0 0-24Zm5.5 17.3a.75.75 0 0 1-1 .25c-2.8-1.7-6.3-2.1-10.4-1.1a.75.75 0 1 1-.34-1.46c4.5-1 8.4-.6 11.5 1.3.35.2.46.65.24 1Zm1.5-3.3a.94.94 0 0 1-1.3.3c-3.2-2-8.1-2.6-11.900-1.400a.94.94 0 1 1-.55-1.800c4.300-1.300 9.700-.7 13.400 1.600.44.27.57.85.35 1.300Zm.13-3.400C15.300 8.300 8.900 8.100 5.200 9.200a1.130 1.130 0 1 1-.65-2.170c4.200-1.300 11.300-1 15.700 1.600a1.130 1.130 0 0 1-1.150 1.940Z" />
        </svg>
      );
    case "folder":
      return (
        <svg {...svg} className="shrink-0 text-amber-400">
          <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7Z" />
        </svg>
      );
    case "file":
      return (
        <svg {...svg} className="shrink-0 text-white/60">
          <path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8l-5-5Z" />
          <path d="M14 3v5h5" />
        </svg>
      );
    case "youtube":
      return (
        <svg {...svg} stroke="none" className="shrink-0">
          <rect x="1" y="4.5" width="22" height="15" rx="4.5" fill="#FF0000" />
          <path d="m10 9 5.5 3-5.5 3V9Z" fill="#fff" />
        </svg>
      );
    case "google":
      return (
        <svg {...svg} stroke="none" className="shrink-0">
          <path d="M21.6 12.2c0-.7-.1-1.4-.2-2H12v3.8h5.4a4.6 4.6 0 0 1-2 3v2.5h3.2c1.9-1.7 3-4.3 3-7.3Z" fill="#4285F4" />
          <path d="M12 22c2.7 0 5-.9 6.6-2.5l-3.2-2.5c-.9.6-2 1-3.4 1-2.6 0-4.8-1.8-5.6-4.1H3.1v2.6A10 10 0 0 0 12 22Z" fill="#34A853" />
          <path d="M6.4 13.9a6 6 0 0 1 0-3.8V7.5H3.1a10 10 0 0 0 0 9l3.3-2.6Z" fill="#FBBC05" />
          <path d="M12 6c1.5 0 2.8.5 3.8 1.5l2.9-2.9A10 10 0 0 0 3.1 7.5l3.3 2.6C7.2 7.8 9.4 6 12 6Z" fill="#EA4335" />
        </svg>
      );
    case "instagram":
      return (
        <svg {...svg} stroke="none" className="shrink-0">
          <defs>
            <linearGradient id="ig-grad" x1="0" y1="1" x2="1" y2="0">
              <stop offset="0" stopColor="#FEDA75" />
              <stop offset=".35" stopColor="#FA7E1E" />
              <stop offset=".6" stopColor="#D62976" />
              <stop offset="1" stopColor="#4F5BD5" />
            </linearGradient>
          </defs>
          <rect x="2" y="2" width="20" height="20" rx="6" fill="url(#ig-grad)" />
          <circle cx="12" cy="12" r="4.2" fill="none" stroke="#fff" strokeWidth="1.8" />
          <circle cx="17.3" cy="6.7" r="1.2" fill="#fff" />
        </svg>
      );
    case "url":
      return (
        <svg {...svg} className="shrink-0 text-sky-300">
          <circle cx="12" cy="12" r="9" />
          <path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18" />
        </svg>
      );
    case "calc":
      return (
        <svg {...svg} className="shrink-0 text-orange-300">
          <rect x="4" y="2.5" width="16" height="19" rx="3" />
          <path d="M8 6.5h8M8 11h.01M12 11h.01M16 11h.01M8 14.5h.01M12 14.5h.01M16 14.5v3.5M8 18h.01M12 18h.01" />
        </svg>
      );
    case "system":
      return (
        <svg {...svg} className="shrink-0 text-rose-300">
          <path d="M12 3v8" />
          <path d="M7.5 6.3a7 7 0 1 0 9 0" />
        </svg>
      );
    case "bookmark":
      return (
        <svg {...svg} className="shrink-0 text-yellow-300">
          <path d="m12 3 2.7 5.6 6.1.9-4.4 4.3 1 6.1L12 17l-5.4 2.9 1-6.1-4.4-4.3 6.1-.9L12 3Z" />
        </svg>
      );
    case "tab":
      return (
        <svg {...svg} className="shrink-0 text-sky-300">
          <rect x="3" y="5" width="18" height="15" rx="2.5" />
          <path d="M3 9.5h18M7 5v4.5" />
        </svg>
      );
    case "action":
      return (
        <svg {...svg} className="shrink-0 text-violet-300">
          <path d="M13 2 4 14h7l-1 8 9-12h-7l1-8Z" />
        </svg>
      );
    case "recent":
      return (
        <svg {...svg} className="shrink-0 text-white/60">
          <circle cx="12" cy="12" r="9" />
          <path d="M12 7v5l3 2" />
        </svg>
      );
    case "app":
      return (
        <svg {...svg} className="shrink-0 text-sky-400">
          <rect x="3" y="3" width="18" height="18" rx="4" />
          <path d="M9 9h6v6H9z" />
        </svg>
      );
    default:
      return (
        <svg {...svg} className="shrink-0 text-white/40">
          <circle cx="11" cy="11" r="7" />
          <path d="m20 20-3.5-3.5" />
        </svg>
      );
  }
}
