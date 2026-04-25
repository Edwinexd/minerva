import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import React from "react"

import { api } from "@/lib/api"
import {
  courseKnowledgeGraphQuery,
  type KnowledgeGraph,
  type KnowledgeGraphEdge,
  type KnowledgeGraphNode,
} from "@/lib/queries"
import { useApiErrorMessage } from "@/lib/use-api-error"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Skeleton } from "@/components/ui/skeleton"

/// Render the course knowledge graph: every classified document as a
/// node colored by `kind`, every linker-asserted edge between them.
///
/// Implementation choice: pure SVG with a small force-directed
/// simulation. No graph library -- React 19 + small node counts (the
/// linker caps at 300 docs per call) make a hand-rolled simulation
/// cheaper than pulling in @xyflow/react or cytoscape and adapting
/// their styling to the kind palette.
export function KnowledgeGraphPage({
  useParams,
}: {
  useParams: () => { courseId: string }
}) {
  const { courseId } = useParams()
  const { t } = useTranslation("teacher")
  const formatError = useApiErrorMessage()
  const queryClient = useQueryClient()
  const { data, isLoading, error } = useQuery(courseKnowledgeGraphQuery(courseId))

  const rebuildMutation = useMutation({
    mutationFn: () =>
      api.post<{ edges: number }>(
        `/courses/${courseId}/documents/knowledge-graph/rebuild`,
        {},
      ),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "knowledge-graph"],
      })
    },
  })

  return (
    <Card>
      <CardHeader>
        <div className="flex items-start justify-between gap-4">
          <div>
            <CardTitle>{t("knowledgeGraph.title")}</CardTitle>
            <CardDescription>{t("knowledgeGraph.description")}</CardDescription>
          </div>
          <Button
            variant="outline"
            onClick={() => rebuildMutation.mutate()}
            disabled={rebuildMutation.isPending}
            title={t("knowledgeGraph.rebuildTitle")}
          >
            {rebuildMutation.isPending
              ? t("knowledgeGraph.rebuilding")
              : t("knowledgeGraph.rebuild")}
          </Button>
        </div>
      </CardHeader>
      <CardContent className="space-y-4">
        {rebuildMutation.isError && (
          <p className="text-sm text-destructive">
            {formatError(rebuildMutation.error)}
          </p>
        )}
        {isLoading ? (
          <Skeleton className="h-[500px] w-full" />
        ) : error || !data ? (
          <p className="text-sm text-destructive">{formatError(error)}</p>
        ) : data.nodes.length === 0 ? (
          <EmptyState message={t("knowledgeGraph.noDocuments")} />
        ) : !data.edges_computed ? (
          <EmptyState
            message={t("knowledgeGraph.notBuiltYet")}
            cta={t("knowledgeGraph.rebuild")}
            onCta={() => rebuildMutation.mutate()}
            disabled={rebuildMutation.isPending}
          />
        ) : (
          <>
            <Legend />
            <GraphSvg graph={data} />
            <EdgeList edges={data.edges} nodes={data.nodes} />
          </>
        )}
      </CardContent>
    </Card>
  )
}

function EmptyState({
  message,
  cta,
  onCta,
  disabled,
}: {
  message: string
  cta?: string
  onCta?: () => void
  disabled?: boolean
}) {
  return (
    <div className="flex flex-col items-center justify-center gap-3 rounded border border-dashed py-16">
      <p className="text-sm text-muted-foreground">{message}</p>
      {cta && (
        <Button onClick={onCta} disabled={disabled}>
          {cta}
        </Button>
      )}
    </div>
  )
}

// ── Kind color palette ─────────────────────────────────────────────
//
// Matches the badge variant logic in documents-page.tsx so a teacher
// scanning the graph sees the same color for assignment_brief here as
// they did on the document list.

const KIND_COLORS: Record<string, { fill: string; stroke: string }> = {
  lecture: { fill: "#3b82f6", stroke: "#1d4ed8" },
  reading: { fill: "#10b981", stroke: "#047857" },
  assignment_brief: { fill: "#ef4444", stroke: "#b91c1c" },
  sample_solution: { fill: "#a855f7", stroke: "#7e22ce" },
  lab_brief: { fill: "#f97316", stroke: "#c2410c" },
  exam: { fill: "#dc2626", stroke: "#991b1b" },
  syllabus: { fill: "#6b7280", stroke: "#374151" },
  unknown: { fill: "#9ca3af", stroke: "#4b5563" },
}

const NULL_KIND_COLOR = { fill: "#e5e7eb", stroke: "#9ca3af" }

function colorFor(kind: string | null) {
  if (kind == null) return NULL_KIND_COLOR
  return KIND_COLORS[kind] ?? NULL_KIND_COLOR
}

function Legend() {
  const { t } = useTranslation("teacher")
  const items = [
    "lecture",
    "reading",
    "assignment_brief",
    "sample_solution",
    "lab_brief",
    "exam",
    "syllabus",
    "unknown",
  ] as const
  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-2 text-xs">
      {items.map((k) => (
        <div key={k} className="flex items-center gap-1.5">
          <span
            className="inline-block h-3 w-3 rounded-full border"
            style={{
              backgroundColor: KIND_COLORS[k].fill,
              borderColor: KIND_COLORS[k].stroke,
            }}
          />
          <span className="text-muted-foreground">
            {t(`documents.kindLabel.${k}`)}
          </span>
        </div>
      ))}
      <div className="ml-auto flex items-center gap-3 text-muted-foreground">
        <div className="flex items-center gap-1.5">
          <svg width="24" height="2" aria-hidden>
            <line x1="0" y1="1" x2="24" y2="1" stroke="#dc2626" strokeWidth="2" />
          </svg>
          <span>{t("knowledgeGraph.edgeKind.solution_of")}</span>
        </div>
        <div className="flex items-center gap-1.5">
          <svg width="24" height="2" aria-hidden>
            <line
              x1="0"
              y1="1"
              x2="24"
              y2="1"
              stroke="#6b7280"
              strokeWidth="2"
              strokeDasharray="4 3"
            />
          </svg>
          <span>{t("knowledgeGraph.edgeKind.part_of_unit")}</span>
        </div>
      </div>
    </div>
  )
}

// ── Force-directed simulation ──────────────────────────────────────
//
// Tiny in-house simulation: one tick = O(N^2) repulsion + O(E) spring
// + O(N) centering. With N ≤ 300 and E small, that's a few-ms tick
// budget. We run for a fixed iteration count off-screen, then render
// the final positions -- no per-frame React re-render. Drag is added
// on top with a small pointer handler that pins the dragged node.

interface SimNode {
  id: string
  filename: string
  kind: string | null
  kind_confidence: number | null
  kind_locked_by_teacher: boolean
  chunk_count: number | null
  x: number
  y: number
  vx: number
  vy: number
  pinned: boolean
}

interface SimEdge {
  src: string
  dst: string
  relation: KnowledgeGraphEdge["relation"]
  confidence: number
}

const VIEWBOX_W = 900
const VIEWBOX_H = 540
const CENTER_X = VIEWBOX_W / 2
const CENTER_Y = VIEWBOX_H / 2

function runSimulation(nodes: SimNode[], edges: SimEdge[], iterations: number) {
  const repulsion = 18000
  const springLength = 90
  const springK = 0.08
  const centerK = 0.01
  const damping = 0.85

  for (let i = 0; i < iterations; i++) {
    // Repulsion (every pair).
    for (let a = 0; a < nodes.length; a++) {
      for (let b = a + 1; b < nodes.length; b++) {
        const na = nodes[a]
        const nb = nodes[b]
        const dx = na.x - nb.x
        const dy = na.y - nb.y
        const d2 = Math.max(dx * dx + dy * dy, 1)
        const f = repulsion / d2
        const d = Math.sqrt(d2)
        const fx = (f * dx) / d
        const fy = (f * dy) / d
        if (!na.pinned) {
          na.vx += fx
          na.vy += fy
        }
        if (!nb.pinned) {
          nb.vx -= fx
          nb.vy -= fy
        }
      }
    }
    // Spring attraction along edges.
    for (const e of edges) {
      const a = nodes.find((n) => n.id === e.src)
      const b = nodes.find((n) => n.id === e.dst)
      if (!a || !b) continue
      const dx = b.x - a.x
      const dy = b.y - a.y
      const d = Math.max(Math.sqrt(dx * dx + dy * dy), 1)
      const f = springK * (d - springLength) * e.confidence
      const fx = (f * dx) / d
      const fy = (f * dy) / d
      if (!a.pinned) {
        a.vx += fx
        a.vy += fy
      }
      if (!b.pinned) {
        b.vx -= fx
        b.vy -= fy
      }
    }
    // Centering pull.
    for (const n of nodes) {
      if (n.pinned) continue
      n.vx += (CENTER_X - n.x) * centerK
      n.vy += (CENTER_Y - n.y) * centerK
    }
    // Integrate + damping.
    for (const n of nodes) {
      if (n.pinned) continue
      n.x += n.vx
      n.y += n.vy
      n.vx *= damping
      n.vy *= damping
      // Clamp inside viewBox.
      n.x = Math.max(20, Math.min(VIEWBOX_W - 20, n.x))
      n.y = Math.max(20, Math.min(VIEWBOX_H - 20, n.y))
    }
  }
}

function GraphSvg({ graph }: { graph: KnowledgeGraph }) {
  // One-shot simulation: deterministic seed positions (golden-angle
  // distribution on a circle) so re-renders don't reshuffle the
  // graph; runSimulation then settles them.
  const sim = React.useMemo(() => {
    const nodes: SimNode[] = graph.nodes.map((n, i) => {
      const golden = Math.PI * (3 - Math.sqrt(5))
      const angle = i * golden
      const radius = 180
      return {
        id: n.id,
        filename: n.filename,
        kind: n.kind,
        kind_confidence: n.kind_confidence,
        kind_locked_by_teacher: n.kind_locked_by_teacher,
        chunk_count: n.chunk_count,
        x: CENTER_X + Math.cos(angle) * radius,
        y: CENTER_Y + Math.sin(angle) * radius,
        vx: 0,
        vy: 0,
        pinned: false,
      }
    })
    const edges: SimEdge[] = graph.edges.map((e) => ({
      src: e.src_id,
      dst: e.dst_id,
      relation: e.relation,
      confidence: e.confidence,
    }))
    runSimulation(nodes, edges, 250)
    return { nodes, edges }
  }, [graph])

  const [hover, setHover] = React.useState<string | null>(null)
  const nodeById = React.useMemo(() => {
    const m = new Map<string, SimNode>()
    for (const n of sim.nodes) m.set(n.id, n)
    return m
  }, [sim])

  return (
    <div className="overflow-hidden rounded border bg-muted/30">
      <svg
        viewBox={`0 0 ${VIEWBOX_W} ${VIEWBOX_H}`}
        className="block w-full"
        style={{ aspectRatio: `${VIEWBOX_W} / ${VIEWBOX_H}` }}
      >
        {/* Edges first so nodes paint over their endpoints. */}
        {sim.edges.map((e, i) => {
          const a = nodeById.get(e.src)
          const b = nodeById.get(e.dst)
          if (!a || !b) return null
          const stroke = e.relation === "solution_of" ? "#dc2626" : "#6b7280"
          const dash = e.relation === "part_of_unit" ? "4 3" : undefined
          const opacity = 0.4 + 0.5 * e.confidence
          const isAdjacent = hover != null && (e.src === hover || e.dst === hover)
          return (
            <g key={i}>
              <line
                x1={a.x}
                y1={a.y}
                x2={b.x}
                y2={b.y}
                stroke={stroke}
                strokeWidth={isAdjacent ? 2.5 : 1.5}
                strokeDasharray={dash}
                opacity={hover == null || isAdjacent ? opacity : 0.1}
              />
              {/* Direction marker for solution_of: small arrowhead near dst. */}
              {e.relation === "solution_of" && (
                <ArrowHead
                  ax={a.x}
                  ay={a.y}
                  bx={b.x}
                  by={b.y}
                  color={stroke}
                  faded={hover != null && !isAdjacent}
                />
              )}
            </g>
          )
        })}
        {/* Nodes. */}
        {sim.nodes.map((n) => {
          const c = colorFor(n.kind)
          const r = nodeRadius(n)
          const faded = hover != null && hover !== n.id
          return (
            <g
              key={n.id}
              transform={`translate(${n.x}, ${n.y})`}
              onMouseEnter={() => setHover(n.id)}
              onMouseLeave={() => setHover(null)}
              style={{ cursor: "pointer" }}
            >
              <circle
                r={r}
                fill={c.fill}
                stroke={c.stroke}
                strokeWidth={n.kind_locked_by_teacher ? 3 : 1.5}
                opacity={faded ? 0.3 : 1}
              />
              <title>
                {nodeTooltip(n)}
              </title>
              {hover === n.id && (
                <text
                  x={r + 4}
                  y={4}
                  fontSize="12"
                  fill="currentColor"
                  className="pointer-events-none select-none"
                >
                  {truncate(n.filename, 48)}
                </text>
              )}
            </g>
          )
        })}
      </svg>
    </div>
  )
}

/// Node radius scales with chunk_count so fat lecture decks visually
/// dominate over single-page handouts. Solution / brief docs that
/// have zero chunks (sample_solution short-circuits embedding) still
/// get a visible minimum radius.
function nodeRadius(n: SimNode): number {
  const chunks = n.chunk_count ?? 0
  return 8 + Math.min(Math.sqrt(chunks) * 1.2, 14)
}

function nodeTooltip(n: SimNode): string {
  const lines = [n.filename]
  lines.push(n.kind ?? "unclassified")
  if (n.kind_locked_by_teacher) lines.push("(locked by teacher)")
  else if (n.kind_confidence != null) {
    lines.push(`confidence: ${Math.round(n.kind_confidence * 100)}%`)
  }
  if (n.chunk_count != null && n.chunk_count > 0) {
    lines.push(`${n.chunk_count} chunks`)
  }
  return lines.join("\n")
}

function truncate(s: string, max: number): string {
  if (s.length <= max) return s
  return s.slice(0, max - 1) + "\u2026"
}

function ArrowHead({
  ax,
  ay,
  bx,
  by,
  color,
  faded,
}: {
  ax: number
  ay: number
  bx: number
  by: number
  color: string
  faded: boolean
}) {
  // Place arrowhead 14px back from b along the a->b vector so it sits
  // outside the destination node.
  const dx = bx - ax
  const dy = by - ay
  const len = Math.max(Math.sqrt(dx * dx + dy * dy), 1)
  const ux = dx / len
  const uy = dy / len
  const tipX = bx - ux * 14
  const tipY = by - uy * 14
  const wingLen = 6
  const left = {
    x: tipX - ux * wingLen + uy * wingLen,
    y: tipY - uy * wingLen - ux * wingLen,
  }
  const right = {
    x: tipX - ux * wingLen - uy * wingLen,
    y: tipY - uy * wingLen + ux * wingLen,
  }
  return (
    <polygon
      points={`${tipX},${tipY} ${left.x},${left.y} ${right.x},${right.y}`}
      fill={color}
      opacity={faded ? 0.2 : 0.9}
    />
  )
}

// ── Edge list (textual fallback / accessibility) ──────────────────

function EdgeList({
  edges,
  nodes,
}: {
  edges: KnowledgeGraphEdge[]
  nodes: KnowledgeGraphNode[]
}) {
  const { t } = useTranslation("teacher")
  const nameById = React.useMemo(() => {
    const m = new Map<string, string>()
    for (const n of nodes) m.set(n.id, n.filename)
    return m
  }, [nodes])

  if (edges.length === 0) {
    return (
      <p className="text-sm text-muted-foreground">
        {t("knowledgeGraph.noEdges")}
      </p>
    )
  }

  return (
    <details className="rounded border">
      <summary className="cursor-pointer px-3 py-2 text-sm font-medium">
        {t("knowledgeGraph.edgeListTitle", { count: edges.length })}
      </summary>
      <ul className="divide-y text-sm">
        {edges.map((e, i) => (
          <li key={i} className="px-3 py-2">
            <div className="flex items-baseline justify-between gap-3">
              <span className="truncate">
                <span className="font-medium">
                  {nameById.get(e.src_id) ?? e.src_id}
                </span>{" "}
                <span className="text-muted-foreground">
                  {t(`knowledgeGraph.edgeKind.${e.relation}`)}
                </span>{" "}
                <span className="font-medium">
                  {nameById.get(e.dst_id) ?? e.dst_id}
                </span>
              </span>
              <span className="text-xs text-muted-foreground tabular-nums">
                {Math.round(e.confidence * 100)}%
              </span>
            </div>
            {e.rationale && (
              <p className="text-xs italic text-muted-foreground">
                {e.rationale}
              </p>
            )}
          </li>
        ))}
      </ul>
    </details>
  )
}
