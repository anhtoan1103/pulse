"use client";

import { useId, useMemo, useState } from "react";
import type { Check } from "@/lib/api";

const WIDTH = 600;
const HEIGHT = 160;
const PAD = { top: 8, right: 8, bottom: 20, left: 40 };

/**
 * Dependency-free SVG line chart of latency over time, with failed checks
 * marked on the baseline. `checks` comes in newest-first (as the API
 * returns it); this renders oldest → newest, left to right.
 */
export function LatencyChart({ checks }: { checks: Check[] }) {
  const gradientId = useId();
  const [hoverIndex, setHoverIndex] = useState<number | null>(null);

  const chronological = useMemo(() => [...checks].reverse(), [checks]);

  const maxLatency = useMemo(() => {
    const values = chronological.map((c) => c.latency_ms).filter((v): v is number => v !== null);
    return Math.max(1, ...values);
  }, [chronological]);

  if (chronological.length === 0) {
    return (
      <div className="flex h-[160px] items-center justify-center text-sm text-zinc-400 dark:text-zinc-600">
        No checks recorded yet.
      </div>
    );
  }

  const innerWidth = WIDTH - PAD.left - PAD.right;
  const innerHeight = HEIGHT - PAD.top - PAD.bottom;
  const n = chronological.length;
  const xFor = (i: number) => PAD.left + (n === 1 ? innerWidth / 2 : (i / (n - 1)) * innerWidth);
  const yFor = (latency: number) => PAD.top + innerHeight - (latency / maxLatency) * innerHeight;

  // Break the line into contiguous segments of successful, latency-bearing
  // checks — a failed/timeout check shouldn't be interpolated across.
  const segments: { x: number; y: number }[][] = [];
  let current: { x: number; y: number }[] = [];
  chronological.forEach((c, i) => {
    if (c.latency_ms !== null) {
      current.push({ x: xFor(i), y: yFor(c.latency_ms) });
    } else if (current.length) {
      segments.push(current);
      current = [];
    }
  });
  if (current.length) segments.push(current);

  const pathFor = (pts: { x: number; y: number }[]) =>
    pts.map((p, i) => `${i === 0 ? "M" : "L"}${p.x.toFixed(1)},${p.y.toFixed(1)}`).join(" ");

  const hovered = hoverIndex !== null ? chronological[hoverIndex] : null;

  return (
    <div className="relative">
      <svg
        viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
        className="w-full text-zinc-400 dark:text-zinc-600"
        role="img"
        aria-label="Latency over time"
        onMouseLeave={() => setHoverIndex(null)}
      >
        <defs>
          <linearGradient id={gradientId} x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="currentColor" stopOpacity="0.15" className="text-sky-500" />
            <stop offset="100%" stopColor="currentColor" stopOpacity="0" className="text-sky-500" />
          </linearGradient>
        </defs>

        {/* y-axis gridlines + labels */}
        {[0, 0.5, 1].map((f) => {
          const y = PAD.top + innerHeight * (1 - f);
          return (
            <g key={f}>
              <line x1={PAD.left} x2={WIDTH - PAD.right} y1={y} y2={y} stroke="currentColor" strokeOpacity={0.15} />
              <text x={PAD.left - 6} y={y + 3} textAnchor="end" fontSize={9} fill="currentColor">
                {Math.round(maxLatency * f)}ms
              </text>
            </g>
          );
        })}

        {segments.map((seg, i) => (
          <g key={i}>
            {seg.length > 1 && (
              <path
                d={`${pathFor(seg)} L${seg[seg.length - 1].x.toFixed(1)},${(HEIGHT - PAD.bottom).toFixed(1)} L${seg[0].x.toFixed(1)},${(HEIGHT - PAD.bottom).toFixed(1)} Z`}
                fill={`url(#${gradientId})`}
                stroke="none"
              />
            )}
            <path d={pathFor(seg)} fill="none" stroke="currentColor" strokeWidth={1.5} className="text-sky-500" />
          </g>
        ))}

        {/* failed checks: red tick on the baseline */}
        {chronological.map((c, i) =>
          c.success ? null : (
            <line
              key={i}
              x1={xFor(i)}
              x2={xFor(i)}
              y1={HEIGHT - PAD.bottom}
              y2={HEIGHT - PAD.bottom + 6}
              stroke="currentColor"
              strokeWidth={2}
              className="text-red-500"
            />
          ),
        )}

        {/* hover targets */}
        {chronological.map((c, i) => (
          <rect
            key={i}
            x={xFor(i) - innerWidth / n / 2}
            y={0}
            width={Math.max(innerWidth / n, 2)}
            height={HEIGHT}
            fill="transparent"
            onMouseEnter={() => setHoverIndex(i)}
          />
        ))}
        {hoverIndex !== null && (
          <line
            x1={xFor(hoverIndex)}
            x2={xFor(hoverIndex)}
            y1={PAD.top}
            y2={HEIGHT - PAD.bottom}
            stroke="currentColor"
            strokeOpacity={0.3}
          />
        )}
      </svg>

      {hovered && (
        <div className="pointer-events-none absolute top-0 right-0 rounded-md border border-zinc-200 bg-white px-2 py-1 text-xs shadow-sm dark:border-zinc-700 dark:bg-zinc-900">
          <div className="text-zinc-500 dark:text-zinc-400">{new Date(hovered.checked_at).toLocaleString()}</div>
          <div className="font-medium">
            {hovered.success ? `${hovered.latency_ms ?? "?"} ms` : (hovered.error_message ?? "failed")}
          </div>
        </div>
      )}
    </div>
  );
}
