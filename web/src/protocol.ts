// Binary frames carry raw terminal bytes. Text frames carry these JSON messages.
export type ClientMsg =
  | { type: 'resize'; rows: number; cols: number }
  | { type: 'ping' };

export type ServerMsg =
  | { type: 'pong' }
  | { type: 'stopped'; resumable: boolean }
  | { type: 'exit'; code: number }
  | { type: 'error'; message: string };

// What the server's screen analysis returns (GET /api/sessions/{id}/screen).
export interface Choice {
  index: number;
  label: string;
  selected: boolean;
  hotkey: string | null;
}

export interface Prompt {
  kind: 'menu' | 'yes_no';
  question: string;
  context: string[];
  choices: Choice[];
  id: string;
}

export interface Analysis {
  activity: 'working' | 'waiting' | 'idle' | 'unknown';
  prompt: Prompt | null;
  tail: string[];
}
