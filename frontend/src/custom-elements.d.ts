import type { DetailedHTMLProps, HTMLAttributes } from "react"

declare module "react" {
  namespace JSX {
    interface IntrinsicElements {
      "relative-time": DetailedHTMLProps<HTMLAttributes<HTMLElement>, HTMLElement> & {
        datetime?: string
      }
    }
  }
}
