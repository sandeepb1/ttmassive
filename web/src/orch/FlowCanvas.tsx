// The flow chart. Nodes are the flow's agents; edges are who hands work to whom. While a run is shown the
// chart is a read-only picture of it (statuses, live edges); otherwise it is the editor.

import { useCallback, useEffect, useMemo, type MouseEvent } from 'react';
import {
  Background, BackgroundVariant, Controls, MarkerType, ReactFlow, useReactFlow,
  type Connection, type Edge, type FinalConnectionState, type Node, type NodeChange,
} from '@xyflow/react';
import { AgentCard, type CardData } from './AgentCard';
import type { MenuItem, MenuState } from './ContextMenu';
import { addEdge, awaitingApproval, connectionProblem, duplicateNode, edgeLabel, removeNode, roleLabel } from './model';
import type { EdgeKind, Flow, Role, RoleDef, Run } from './types';

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
  /** The edge whose settings are open, if any. */
  selectedEdge: string | null;
  editable: boolean;
  onFlow: (update: (f: Flow) => Flow) => void;
  onSelect: (id: string | null) => void;
  onSelectEdge: (id: string) => void;
  onProblem: (message: string) => void;
  /** Changes when the view should refit the agents (after Tidy, say). */
  fitToken: number;
  /** A different key starts a fresh view that fits the agents: a run opens on its own, not on the editor's zoom. */
  viewKey: string;
  roles: RoleDef[];
  onAddAt: (role: Role, at: { x: number; y: number }) => void;
  onTidy: () => void;
  openMenu: (m: MenuState) => void;
}

export function FlowCanvas({ flow, run, selected, selectedEdge, editable, onFlow, onSelect, onSelectEdge, onProblem, fitToken, viewKey, roles, onAddAt, onTidy, openMenu }: CanvasProps) {
  const rf = useReactFlow();
  useEffect(() => {
    if (!fitToken) return;
    // Give the chart a moment to take the new positions first, or it fits the old ones.
    const id = window.setTimeout(() => void rf.fitView({ padding: 0.08, maxZoom: 1.1, duration: 250 }), 80);
    return () => window.clearTimeout(id);
  }, [fitToken, rf]);

  const awaiting = useMemo(() => new Set(run ? awaitingApproval(run) : []), [run]);

  const nodes: Node[] = useMemo(
    () =>
      flow.nodes.map((n) => ({
        id: n.id,
        type: 'agent',
        position: { x: n.x, y: n.y },
        selected: n.id === selected,
        draggable: editable,
        data: { node: n, run: run?.nodes[n.id], waiting: awaiting.has(n.id), selected: n.id === selected } satisfies CardData,
      })),
    [flow.nodes, run, selected, editable, awaiting],
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
          label: edgeLabel(e, run),
          selected: e.id === selectedEdge,
          markerEnd: { type: MarkerType.ArrowClosed, width: 16, height: 16 },
          deletable: editable,
        } satisfies Edge;
      }),
    [flow.edges, run, editable, selectedEdge],
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

  // React Flow refuses an invalid drop before `onConnect` runs, so say why here: a drop on a handle that was refused.
  const onConnectEnd = useCallback(
    (_: unknown, st: FinalConnectionState) => {
      if (st.isValid !== false || !st.fromNode || !st.toNode || !st.toHandle) return;
      const kind = kindOf({ sourceHandle: st.fromHandle?.id, targetHandle: st.toHandle.id });
      const why = kind ? connectionProblem(flow, st.fromNode.id, st.toNode.id, kind) : 'Connect an output to an input, or a send-back dot to a drop dot';
      if (why) onProblem(why);
    },
    [flow, onProblem],
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

  // ---- right-click menus
  const fit = () => void rf.fitView({ padding: 0.08, maxZoom: 1.1, duration: 250 });

  const onNodeContextMenu = (e: MouseEvent, n: Node) => {
    e.preventDefault();
    onSelect(n.id);
    const items: MenuItem[] = [{ label: 'Details', icon: 'info', onSelect: () => onSelect(n.id) }];
    if (editable) {
      items.push(
        { label: 'Duplicate agent', icon: 'files', onSelect: () => { const d = duplicateNode(flow, n.id); if (d) { onFlow(() => d.flow); onSelect(d.node.id); } } },
        { kind: 'separator' },
        { label: 'Remove agent', icon: 'trash', danger: true, onSelect: () => { onFlow((f) => removeNode(f, n.id)); onSelect(null); } },
      );
    }
    openMenu({ x: e.clientX, y: e.clientY, items });
  };

  const onEdgeContextMenu = (e: MouseEvent, edge: Edge) => {
    e.preventDefault();
    if (!editable) return;
    openMenu({ x: e.clientX, y: e.clientY, items: [{ label: edge.label ? 'Remove send-back' : 'Remove connection', icon: 'trash', danger: true, onSelect: () => onFlow((f) => ({ ...f, edges: f.edges.filter((x) => x.id !== edge.id) })) }] });
  };

  const onPaneContextMenu = (e: MouseEvent | globalThis.MouseEvent) => {
    e.preventDefault();
    const items: MenuItem[] = [];
    if (editable) {
      const at = rf.screenToFlowPosition({ x: e.clientX - 116, y: e.clientY - 40 });
      const add = (r: RoleDef): MenuItem => ({ label: `Add ${r.label}`, icon: 'plus', onSelect: () => onAddAt(r.id, at) });
      items.push(...roles.filter((r) => r.core).map(add), { kind: 'separator' }, ...roles.filter((r) => !r.core).map(add), { kind: 'separator' });
      items.push({ label: 'Tidy up', icon: 'grid', disabled: flow.nodes.length < 2, onSelect: onTidy });
    }
    items.push({ label: 'Fit to screen', icon: 'columns', disabled: flow.nodes.length === 0, onSelect: fit });
    openMenu({ x: e.clientX, y: e.clientY, items });
  };

  return (
    <ReactFlow
      key={viewKey}
      nodes={nodes}
      edges={edges}
      nodeTypes={nodeTypes}
      colorMode="dark"
      fitView
      fitViewOptions={{ padding: 0.08, maxZoom: 1.1 }}
      minZoom={0.25}
      maxZoom={1.75}
      nodesDraggable={editable}
      nodesConnectable={editable}
      edgesFocusable={editable}
      deleteKeyCode={editable ? ['Backspace', 'Delete'] : null}
      connectionRadius={36}
      onNodesChange={onNodesChange}
      onConnect={onConnect}
      onConnectEnd={onConnectEnd}
      isValidConnection={isValid}
      onNodesDelete={onNodesDelete}
      onEdgesDelete={onEdgesDelete}
      onNodeClick={(_: MouseEvent, n: Node) => onSelect(n.id)}
      onEdgeClick={(_: MouseEvent, e: Edge) => onSelectEdge(e.id)}
      onPaneClick={() => onSelect(null)}
      onNodeContextMenu={onNodeContextMenu}
      onEdgeContextMenu={onEdgeContextMenu}
      onPaneContextMenu={onPaneContextMenu}
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
