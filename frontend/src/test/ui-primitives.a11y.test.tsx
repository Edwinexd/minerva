import { describe, expect, it } from "vitest"

import { axe, renderWithProviders } from "./a11y"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Textarea } from "@/components/ui/textarea"
import { Label } from "@/components/ui/label"
import { Checkbox } from "@/components/ui/checkbox"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"

describe("Button a11y", () => {
  const variants = [
    "default",
    "outline",
    "secondary",
    "ghost",
    "destructive",
    "link",
  ] as const

  it.each(variants)("variant %s has no axe violations", async (variant) => {
    const { container } = renderWithProviders(
      <Button variant={variant}>Save changes</Button>
    )
    expect(await axe(container)).toHaveNoViolations()
  })

  it("icon-only button exposes an accessible name", async () => {
    const { container, getByRole } = renderWithProviders(
      <Button size="icon" aria-label="Close dialog">
        <svg aria-hidden="true" focusable="false" />
      </Button>
    )
    expect(getByRole("button", { name: "Close dialog" })).toBeInTheDocument()
    expect(await axe(container)).toHaveNoViolations()
  })
})

describe("Form control a11y", () => {
  it("text input associated with a label has no violations", async () => {
    const { container } = renderWithProviders(
      <div>
        <Label htmlFor="email">Email address</Label>
        <Input id="email" type="email" placeholder="you@example.com" />
      </div>
    )
    expect(await axe(container)).toHaveNoViolations()
  })

  it("textarea associated with a label has no violations", async () => {
    const { container } = renderWithProviders(
      <div>
        <Label htmlFor="bio">Biography</Label>
        <Textarea id="bio" />
      </div>
    )
    expect(await axe(container)).toHaveNoViolations()
  })

  it("checkbox with a label has no violations", async () => {
    const { container } = renderWithProviders(
      <div>
        <Checkbox id="consent" />
        <Label htmlFor="consent">I agree to the terms</Label>
      </div>
    )
    expect(await axe(container)).toHaveNoViolations()
  })
})

describe("Card a11y", () => {
  it("card with heading and content has no violations", async () => {
    const { container } = renderWithProviders(
      <Card>
        <CardHeader>
          <CardTitle>Course settings</CardTitle>
          <CardDescription>Manage how this course behaves.</CardDescription>
        </CardHeader>
        <CardContent>
          <p>Settings content.</p>
        </CardContent>
      </Card>
    )
    expect(await axe(container)).toHaveNoViolations()
  })
})
