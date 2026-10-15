// The only file the page loads for orchestrator mode; it brings React and React Flow with it.

import '@xyflow/react/dist/style.css';
import { createRoot } from 'react-dom/client';
import { App } from './App';

export function mountOrch(root: HTMLElement, opts: { initialRun: string | null; onExit: () => void; onSession: (id: string) => void; onRoute: (runId: string | null) => void }) {
  createRoot(root).render(<App {...opts} />);
}
