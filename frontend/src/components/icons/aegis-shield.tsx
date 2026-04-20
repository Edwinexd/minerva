import { forwardRef, type SVGProps } from "react"

import { cn } from "@/lib/utils"

export interface AegisShieldProps extends Omit<SVGProps<SVGSVGElement>, "ref"> {
  size?: number | string
  strokeWidth?: number | string
}

/**
 * Aegis: Minerva's shield.
 *
 * Used to represent prompt-safety / guarded-input affordances (e.g. next to
 * the chat send button). Shape is an owl-shield silhouette: top corners
 * pinch into ear-tufts, sides bulge into a facial disc, bottom tapers like
 * a hoplon. Two ring eyes inside make the Minerva owl reference explicit
 * without getting busy at 16–20 px.
 *
 * API mirrors lucide-react icons: `size`, `strokeWidth`, `color`, and any
 * other SVG props pass through. Uses `currentColor` so Tailwind `text-*`
 * classes control both the body and the eyes.
 */
export const AegisShield = forwardRef<SVGSVGElement, AegisShieldProps>(
  (
    { size = 24, strokeWidth = 2, className, color = "currentColor", ...props },
    ref,
  ) => (
    <svg
      ref={ref}
      xmlns="http://www.w3.org/2000/svg"
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke={color}
      strokeWidth={strokeWidth}
      strokeLinecap="round"
      strokeLinejoin="round"
      className={cn("lucide", className)}
      aria-hidden="true"
      {...props}
    >
      <path d="M4 3.2 6 5c2-1.3 4-1.3 6 0s4 1.3 6 0l2-1.8V11c0 5.5-4 9.5-8 11-4-1.5-8-5.5-8-11z" />
      <circle cx="9.5" cy="11" r="1.3" strokeWidth="1.2" />
      <circle cx="14.5" cy="11" r="1.3" strokeWidth="1.2" />
    </svg>
  ),
)

AegisShield.displayName = "AegisShield"
