// How the app decides what to show when it is opened: from the home-screen icon, from a notification, from a
// shortcut, or from a link with #session. Pure, so node can test it; main.ts does the opening.

const SESSION_ID = /^[A-Za-z0-9_-]{1,64}$/;

export const isSessionId = (s: string) => SESSION_ID.test(s);

/** What a launch URL asks for. */
export interface Launch {
  /** The session named by the address (`#name`), when it is a valid name. */
  session: string | null;
  /** `?new=claude` or `?new=codex`: open the new-session sheet with that agent chosen. */
  newAgent: 'claude' | 'codex' | null;
  /** `?resume`: open the conversation list of the focused session. */
  resume: boolean;
}

export function parseLaunch(url: string, base = 'http://localhost/'): Launch {
  let u: URL;
  try {
    u = new URL(url, base);
  } catch {
    return { session: null, newAgent: null, resume: false };
  }
  let name = '';
  try {
    name = decodeURIComponent(u.hash.slice(1));
  } catch {
    /* a broken escape names no session */
  }
  const agent = u.searchParams.get('new');
  return {
    session: isSessionId(name) ? name : null,
    newAgent: agent === 'claude' || agent === 'codex' ? agent : null,
    resume: u.searchParams.has('resume'),
  };
}

/**
 * The session to open at start. The address wins. An installed app is opened from the icon with no address at all,
 * so then it goes back to where you were, as a native app would - but only if that session still exists, because
 * the server would create an empty one for a name it does not know. `known` is null when the list could not be read.
 */
export function startSession(fromAddress: string | null, last: string | null, known: string[] | null): string {
  if (fromAddress) return fromAddress;
  if (last && isSessionId(last) && known?.includes(last)) return last;
  return 'main';
}

/** Whether two launches are the same request arriving twice (the page's own URL and the launch queue). */
export function sameLaunch(a: Launch, b: Launch): boolean {
  return a.session === b.session && a.newAgent === b.newAgent && a.resume === b.resume;
}
