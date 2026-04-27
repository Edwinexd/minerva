import { forwardRef, useId, type SVGProps } from "react"

import { cn } from "@/lib/utils"

export interface AegisShieldFilledProps
  extends Omit<SVGProps<SVGSVGElement>, "ref"> {
  size?: number | string
}

/**
 * Aegis: colored tile variant of the shield logo.
 *
 * Mirrors the Minerva favicon's visual language; a rounded-rect
 * tile with a radial blue gradient background and a white-masked
 * foreground shape; so the Feedback panel reads as part of the
 * Minerva product, not a stuck-on third-party widget. The owl
 * favicon's gradient palette is reused here verbatim (`#bff0ff`
 * highlight -> `#3b86bf` mid -> `#083561` shadow -> `#042448` deep)
 * with the same off-axis spotlight overlay.
 *
 * Foreground is the shield silhouette + two ring eyes (the same
 * geometry as the line variant in `aegis-shield.tsx`), filled
 * with `#fff` so the cutout reads as masking against the gradient.
 *
 * Two SVG ids per render are scoped via `useId` so multiple
 * instances of the icon on the same page don't clash on the `<defs>`
 * (a frequent React-icon footgun; two `<radialGradient id="g1">`
 * blocks would cause every later one to inherit the first).
 *
 * Sizing API mirrors lucide-react: pass `size` (number or CSS
 * string), other SVG props pass through. No `color`/strokeWidth
 * since the fill palette is fixed.
 */
export const AegisShieldFilled = forwardRef<
  SVGSVGElement,
  AegisShieldFilledProps
>(({ size = 24, className, ...props }, ref) => {
  const uid = useId()
  const gradId = `aegis-shield-grad-${uid}`
  const overlayId = `aegis-shield-overlay-${uid}`
  const clipId = `aegis-shield-clip-${uid}`
  return (
    <svg
      ref={ref}
      xmlns="http://www.w3.org/2000/svg"
      width={size}
      height={size}
      viewBox="0 0 24 24"
      className={cn(className)}
      aria-hidden="true"
      {...props}
    >
      <defs>
        <radialGradient
          id={gradId}
          cx="3"
          cy="20"
          r="26"
          gradientUnits="userSpaceOnUse"
        >
          <stop offset="0" stopColor="#bff0ff" />
          <stop offset="0.22" stopColor="#3b86bf" />
          <stop offset="0.55" stopColor="#083561" />
          <stop offset="1" stopColor="#042448" />
        </radialGradient>
        <radialGradient
          id={overlayId}
          cx="22"
          cy="2"
          r="20"
          gradientUnits="userSpaceOnUse"
        >
          <stop offset="0" stopColor="#a6d8f0" stopOpacity="0.9" />
          <stop offset="0.5" stopColor="#1d547f" stopOpacity="0.25" />
          <stop offset="1" stopColor="#083561" stopOpacity="0" />
        </radialGradient>
        <clipPath id={clipId}>
          {/* Rounded square tile; ~18%-radius corners to match
              the favicon's cushioned feel without going full pill. */}
          <rect x="0.5" y="0.5" width="23" height="23" rx="4.5" ry="4.5" />
        </clipPath>
      </defs>
      {/* Tile background. Two stacked gradient rects: the base
          deep-to-light radial + an off-axis highlight overlay,
          composited inside the rounded clip. */}
      <g clipPath={`url(#${clipId})`}>
        <rect x="0" y="0" width="24" height="24" fill={`url(#${gradId})`} />
        <rect x="0" y="0" width="24" height="24" fill={`url(#${overlayId})`} />
      </g>
      {/* White-masked foreground: shield silhouette + two ring eyes.
          Same geometry as the line variant; only the rendering
          changes (fill instead of stroke). The shield path is
          slightly inset from the tile edge so the gradient frames
          it cleanly. */}
      <g fill="#ffffff">
        <path d="M5.5 5 7 6.4c1.5-1 3-1 4.5 0s3 1 4.5 0L17.5 5v4.6c0 4.4-3 7.6-5.5 8.8-2.5-1.2-5.5-4.4-5.5-8.8z" />
      </g>
      {/* Eyes punched as the gradient color so they read as cutouts
          rather than additional white blobs. Using the deepest
          shadow stop makes them legible at small sizes. */}
      <g fill="#042448">
        <circle cx="10.3" cy="10.5" r="0.85" />
        <circle cx="13.7" cy="10.5" r="0.85" />
      </g>
    </svg>
  )
})

AegisShieldFilled.displayName = "AegisShieldFilled"
