/**
 * One entry of the desktop navigation rail.
 *
 * Extracted so the export dialog, the Codex dialog and the plain buttons all render the
 * same geometry: a rail that sized itself per-item would drift the moment one label got
 * longer, and the buttons live in different files.
 */
import React from 'react';

export interface NavItemProps {
  label: string;
  title: string;
  icon: React.ReactNode;
  onClick?: () => void;
  disabled?: boolean;
  primary?: boolean;
}

export function NavItem({ label, title, icon, onClick, disabled, primary }: NavItemProps) {
  return (
    <button
      type="button"
      className={`mf-nav__item${primary ? ' is-primary' : ''}${disabled ? ' is-disabled' : ''}`}
      title={title}
      aria-label={title}
      disabled={disabled}
      onClick={onClick}
    >
      <span className="mf-nav__icon" aria-hidden>{icon}</span>
      <span className="mf-nav__label">{label}</span>
    </button>
  );
}

/* 16px stroke icons, drawn inline: the overlay cannot import the app's font-awesome bundle
   without pulling in its build graph, and a dependency for four glyphs is not worth it. */
const stroke = {
  fill: 'none',
  stroke: 'currentColor',
  strokeWidth: 1.6,
  strokeLinecap: 'round' as const,
  strokeLinejoin: 'round' as const,
};

export const Icons = {
  server: (
    <svg viewBox="0 0 24 24" width="20" height="20" {...stroke}>
      <rect x="3" y="4" width="18" height="7" rx="2" />
      <rect x="3" y="13" width="18" height="7" rx="2" />
      <path d="M7 7.5h.01M7 16.5h.01" />
    </svg>
  ),
  export: (
    <svg viewBox="0 0 24 24" width="20" height="20" {...stroke}>
      <path d="M12 3v10" />
      <path d="M8 9l4 4 4-4" />
      <path d="M4 17v2a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-2" />
    </svg>
  ),
  codex: (
    <svg viewBox="0 0 24 24" width="20" height="20" {...stroke}>
      <path d="M12 3l2.2 4.6L19 9l-3.4 3.5.8 4.9L12 15l-4.4 2.4.8-4.9L5 9l4.8-1.4z" />
    </svg>
  ),
  settings: (
    <svg viewBox="0 0 24 24" width="20" height="20" {...stroke}>
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.7 1.7 0 0 0 .3 1.9l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.9-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.9.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.9 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.9l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.9.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.9-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.9V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z" />
    </svg>
  ),
};
