// The flow chart. Nodes are the flow's agents; edges are who hands work to whom. While a run is shown the
// chart is a read-only picture of it (statuses, live edges); otherwise it is the editor.

import { useCallback, useEffect, useMemo, type MouseEvent } from 'react';
import {
  Background, BackgroundVariant, Controls, MarkerType, ReactFlow, useReactFlow,
  type Connection, type Edge, type Node, type NodeChange,
} from '@xyflow/react';
import { AgentCard, type CardData } from './AgentCard';
import { addEdge, connectionProblem } from './model';
import type { EdgeKind, Flow, Run } from './types';

const nodeTypes = { agent: AgentCard };

/** Which kind of edge a drag makes: the judge's bottom handle sends work back; everything else hands it on. */
function kindOf(c: { sourceHandle?: string | null; targetHandle?: string | null }): EdgeKind | null {
  const back = c.sourceHandle === 'back-out';
  if (back !== (c.targetHandle === 'back-in')) return null; // the two ends must match
  return back ? 'retry' : 'forward';
}

export interface CanvasProps {
  flow: Flow;
  run: Run | null;
  selected: string | null;
  editable: boolean;
  onFlow: (update: (f: Flow) => Flow) => void;
  onSelect: (id: string | null) => void;
  onProblem: (message: string) => void;
  /** Changes when the view should refit the agents (after Tidy, say). */
  fitToken: number;
  /** A different key starts a fresh view that fits the agents: a run opens on its own, not on the editor's zoom. */
  viewKey: string;
}

export function FlowCanvas({ flow, run, selected, editable, onFlow, onSelect, onProblem, fitToken, viewKey }: CanvasProps) {
  const rf = useReactFlow();
  useEffect(() => {
    if (!fitToken) return;
    // Give the chart a moment to take the new positions first, or it fits the old ones.
    const id = window.setTimeout(() => void rf.fitView({ padding: 0.12, maxZoom: 1.1, duration: 250 }), 80);
    return () => window.clearTimeout(id);
  }, [fitToken, rf]);

  const waitingFor = run?.status.state === 'paused' && run.status.pause.kind === 'approval' ? run.status.pause.node : null;

  const nodes: Node[] = useMemo(
    () =>
      flow.nodes.map((n) => ({
        id: n.id,
        type: 'agent',
        position: { x: n.x, y: n.y },
        selected: n.id === selected,
        draggable: editable,
        data: { node: n, run: run?.nodes[n.id], waiting: n.id === waitingFor, selected: n.id === selected } satisfies CardData,
      })),
    [flow.nodes, run, selected, editable, waitingFor],
  );

  const edges: Edge[] = useMemo(
    () =>
      flow.edges.map((e) => {
        const retry = e.kind === 'retry';
        const from = run?.nodes[e.from]?.status;
        const to = run?.nodes[e.to]?.status;
        const state = retry ? 'retry' : from === 'done' && to === 'running' ? 'live' : from === 'done' && to === 'done' ? 'done' : '';
        return {
          id: e.id,
          source: e.from,
          target: e.to,
          sourceHandle: retry ? 'back-out' : 'out',
          targetHandle: retry ? 'back-in' : 'in',
          className: state ? `mc-edge-${state}` : undefined,
          label: retry ? 'send back' : undefined,
          markerEnd: { type: MarkerType.ArrowClosed, width: 16, height: 16 },
          deletable: editable,
        } satisfies Edge;
      }),
    [flow.edges, run, editable],
  );

  const onNodesChange = useCallback(
    (changes: NodeChange[]) => {
      for (const c of changes) {
        if (c.type === 'position' && c.position) {
          const { x, y } = c.position;
          onFlow((f) => ({ ...f, nodes: f.nodes.map((n) => (n.id === c.id ? { ...n, x: Math.round(x), y: Math.round(y) } : n)) }));
        }
      }
    },
    [onFlow],
  );

  const isValid = useCallback(
    (c: Connection | Edge) => {
      const kind = kindOf(c);
      return !!kind && connectionProblem(flow, c.source, c.target, kind) === null;
    },
    [flow],
  );

  const onConnect = useCallback(
    (c: Connection) => {
      const kind = kindOf(c);
      const why = kind ? connectionProblem(flow, c.source, c.target, kind) : 'Connect an output to an input';
      if (why || !kind) return onProblem(why ?? 'That connection is not allowed');
      onFlow((f) => addEdge(f, c.source, c.target, kind));
    },
    [flow, onFlow, onProblem],
  );

  const onNodesDelete = useCallback(
    (gone: Node[]) => {
      const ids = new Set(gone.map((n) => n.id));
      onFlow((f) => ({ ...f, nodes: f.nodes.filter((n) => !ids.has(n.id)), edges: f.edges.filter((e) => !ids.has(e.from) && !ids.has(e.to)) }));
      onSelect(null);
    },
    [onFlow, onSelect],
  );

  const onEdgesDelete = useCallback(
    (gone: Edge[]) => {
      const ids = new Set(gone.map((e) => e.id));
      onFlow((f) => ({ ...f, edges: f.edges.filter((e) => !ids.has(e.id)) }));
    },
    [onFlow],
  );

  return (
    <ReactFlow
      key={viewKey}
      nodes={nodes}
      edges={edges}
      nodeTypes={nodeTypes}
      colorMode="dark"
      fitView
      fitViewOptions={{ padding: 0.12, maxZoom: 1.1 }}
      minZoom={0.25}
      maxZoom={1.75}
      nodesDraggable={editable}
      nodesConnectable={editable}
      edgesFocusable={editable}
      deleteKeyCode={editable ? ['Backspace', 'Delete'] : null}
      connectionRadius={36}
      onNodesChange={onNodesChange}
      onConnect={onConnect}
      isValidConnection={isValid}
      onNodesDelete={onNodesDelete}
      onEdgesDelete={onEdgesDelete}
      onNodeClick={(_: MouseEvent, n: Node) => onSelect(n.id)}
      onPaneClick={() => onSelect(null)}
    >
      <Background variant={BackgroundVariant.Dots} gap={22} size={1.5} />
      <Controls showInteractive={false} position="bottom-right" />
    </ReactFlow>
  );
}

/** Add an agent where the user is looking. Needs to be inside the provider. */
export function useViewportCenter() {
  const rf = useReactFlow();
  return useCallback(
    (index: number) => {
      const el = document.querySelector('.mc-stage');
      const r = el?.getBoundingClientRect();
      const p = rf.screenToFlowPosition({ x: (r?.left ?? 0) + (r?.width ?? 600) / 2, y: (r?.top ?? 0) + (r?.height ?? 400) / 2 });
      // Each new one sits a little off the last so they do not stack exactly.
      return { x: p.x - 116 + (index % 5) * 26, y: p.y - 60 + (index % 5) * 26 };
    },
    [rf],
  );
}
