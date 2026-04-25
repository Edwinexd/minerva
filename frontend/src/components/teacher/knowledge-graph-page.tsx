import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import React from "react"
import ForceGraph2D from "react-force-graph-2d"

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
/// Renderer: react-force-graph-2d (vasturiano), which wraps d3-force in
/// a canvas-based React component. Handles drag, zoom, hover, layout
/// stability and high node counts out of the box. We previously had a
/// hand-rolled SVG simulation here; that worked for ~50 nodes but
/// reinvented half of d3-force and got brittle on edge cases (very
/// dense subgraphs, hover with many adjacent edges, etc.). Switched
/// to the established library so the rendering is one fewer thing to
/// worry about and so future work goes into the data model, not a
/// custom layout engine.
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
            <ForceGraphCanvas graph={data} />
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

function colorFor(kind: string | null | undefined) {
  if (kind == null) return NULL_KIND_COLOR
  return KIND_COLORS[kind] ?? NULL_KIND_COLOR
}

const EDGE_COLOR = {
  solution_of: "#dc2626",
  part_of_unit: "#6b7280",
} as const

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
            <line
              x1="0"
              y1="1"
              x2="24"
              y2="1"
              stroke={EDGE_COLOR.solution_of}
              strokeWidth="2"
            />
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
              stroke={EDGE_COLOR.part_of_unit}
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

// ── Force-directed canvas ──────────────────────────────────────────
//
// Adapter: convert KnowledgeGraph (server payload) into the shape
// react-force-graph expects, and pass through styling. The library
// mutates node/link objects in place to track simulation state, so
// we deep-clone via JSON round-trip on each render that introduces
// new data; otherwise we'd accumulate stale x/y/vx/vy from previous
// graphs when the user clicks Rebuild.

interface ForceNode {
  id: string
  filename: string
  kind: string | null
  kindConfidence: number | null
  kindLocked: boolean
  chunkCount: number | null
  // Simulation fields populated by react-force-graph in place.
  x?: number
  y?: number
  vx?: number
  vy?: number
}

interface ForceLink {
  source: string | ForceNode
  target: string | ForceNode
  relation: KnowledgeGraphEdge["relation"]
  confidence: number
  rationale: string | null
}

function nodeRadius(n: ForceNode): number {
  const chunks = n.chunkCount ?? 0
  return 6 + Math.min(Math.sqrt(chunks) * 1.2, 12)
}

function ForceGraphCanvas({ graph }: { graph: KnowledgeGraph }) {
  // Build adapter shape. Re-derive whenever the server payload
  // changes so a Rebuild click picks up the new edges.
  const data = React.useMemo(
    () => ({
      nodes: graph.nodes.map(
        (n): ForceNode => ({
          id: n.id,
          filename: n.filename,
          kind: n.kind,
          kindConfidence: n.kind_confidence,
          kindLocked: n.kind_locked_by_teacher,
          chunkCount: n.chunk_count,
        }),
      ),
      links: graph.edges.map(
        (e): ForceLink => ({
          source: e.src_id,
          target: e.dst_id,
          relation: e.relation,
          confidence: e.confidence,
          rationale: e.rationale,
        }),
      ),
    }),
    [graph],
  )

  // Track container size so the canvas fills available width and
  // resizes with the panel. The library does not auto-resize on
  // its own.
  const containerRef = React.useRef<HTMLDivElement | null>(null)
  const [size, setSize] = React.useState({ width: 900, height: 540 })
  React.useEffect(() => {
    const el = containerRef.current
    if (!el) return
    const ro = new ResizeObserver((entries) => {
      for (const entry of entries) {
        const { width } = entry.contentRect
        // Maintain a 5:3 aspect ratio, capped at 720 tall.
        setSize({ width, height: Math.min(720, Math.max(360, width * 0.6)) })
      }
    })
    ro.observe(el)
    return () => ro.disconnect()
  }, [])

  // Hover state to fade non-adjacent nodes/edges.
  const [hoverId, setHoverId] = React.useState<string | null>(null)
  const adjacent = React.useMemo(() => {
    if (hoverId == null) return null
    const ids = new Set<string>([hoverId])
    for (const link of data.links) {
      const src =
        typeof link.source === "object" ? link.source.id : link.source
      const dst =
        typeof link.target === "object" ? link.target.id : link.target
      if (src === hoverId) ids.add(dst)
      if (dst === hoverId) ids.add(src)
    }
    return ids
  }, [hoverId, data.links])

  return (
    <div
      ref={containerRef}
      className="overflow-hidden rounded border bg-muted/30"
      style={{ height: size.height }}
    >
      <ForceGraph2D
        graphData={data}
        width={size.width}
        height={size.height}
        backgroundColor="rgba(0,0,0,0)"
        // ── Node rendering ─────────────────────────────────────
        nodeRelSize={1}
        nodeVal={(n) => {
          const r = nodeRadius(n as ForceNode)
          // Library uses sqrt(val) for radius; we want our own radius
          // formula, so feed val = r^2 and let the lib do sqrt.
          return r * r
        }}
        nodeColor={(n) => colorFor((n as ForceNode).kind).fill}
        nodeLabel={(n) => nodeTooltip(n as ForceNode)}
        onNodeHover={(n) => setHoverId(n ? (n as ForceNode).id : null)}
        nodeCanvasObjectMode={() => "after"}
        nodeCanvasObject={(n, ctx) => {
          const node = n as ForceNode
          if (node.x == null || node.y == null) return
          const r = nodeRadius(node)
          const c = colorFor(node.kind)
          const faded = adjacent != null && !adjacent.has(node.id)
          // Outline (extra thick when teacher-locked).
          ctx.beginPath()
          ctx.arc(node.x, node.y, r, 0, 2 * Math.PI)
          ctx.lineWidth = node.kindLocked ? 2.5 : 1
          ctx.strokeStyle = c.stroke
          ctx.globalAlpha = faded ? 0.3 : 1
          ctx.stroke()
          // Hover label: filename next to the node.
          if (hoverId === node.id) {
            ctx.font = "12px system-ui, sans-serif"
            ctx.textBaseline = "middle"
            ctx.fillStyle = c.stroke
            ctx.globalAlpha = 1
            ctx.fillText(truncate(node.filename, 48), node.x + r + 4, node.y)
          }
          ctx.globalAlpha = 1
        }}
        // ── Edge rendering ─────────────────────────────────────
        linkColor={(l) => EDGE_COLOR[(l as ForceLink).relation]}
        linkWidth={(l) => {
          const link = l as ForceLink
          const src =
            typeof link.source === "object" ? link.source.id : link.source
          const dst =
            typeof link.target === "object" ? link.target.id : link.target
          const isAdj =
            adjacent != null && (adjacent.has(src) || adjacent.has(dst))
          return isAdj ? 2.5 : 1.2
        }}
        linkLineDash={(l) =>
          (l as ForceLink).relation === "part_of_unit" ? [4, 3] : null
        }
        linkDirectionalArrowLength={(l) =>
          (l as ForceLink).relation === "solution_of" ? 6 : 0
        }
        linkDirectionalArrowRelPos={1}
        linkLabel={(l) => edgeTooltip(l as ForceLink)}
        // ── Layout tuning ──────────────────────────────────────
        cooldownTicks={120}
        d3AlphaDecay={0.03}
        d3VelocityDecay={0.3}
      />
    </div>
  )
}

function nodeTooltip(n: ForceNode): string {
  const lines: string[] = [n.filename]
  lines.push(n.kind ?? "unclassified")
  if (n.kindLocked) {
    lines.push("(locked by teacher)")
  } else if (n.kindConfidence != null) {
    lines.push(`confidence: ${Math.round(n.kindConfidence * 100)}%`)
  }
  if (n.chunkCount != null && n.chunkCount > 0) {
    lines.push(`${n.chunkCount} chunks`)
  }
  // react-force-graph uses HTML for labels; basic line break.
  return lines.join("<br>")
}

function edgeTooltip(l: ForceLink): string {
  const lines: string[] = [
    l.relation === "solution_of" ? "is a solution to" : "is in the same unit as",
    `confidence: ${Math.round(l.confidence * 100)}%`,
  ]
  if (l.rationale) lines.push(l.rationale)
  return lines.join("<br>")
}

function truncate(s: string, max: number): string {
  if (s.length <= max) return s
  return s.slice(0, max - 1) + "\u2026"
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
