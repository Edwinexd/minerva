"""Model-independent half of visual extraction: find the slides in a lecture
video, render PDF pages, and turn grounded OCR output into labelled blocks,
slide text, page numbers and figure crops with their context.

Play's studio recordings cut between full-screen slides and a composite
with the presenter beside the slide, so detection watches the full frame
and over-splits; adjacent segments are merged after OCR by page number or
word overlap.
"""

import ast
import re
from collections import Counter

import attr
import av
import numpy as np
from PIL import Image

# Grounded output tags each block as <|ref|>label<|/ref|><|det|>[[x0,y0,x1,y1]]<|/det|>
# (or <|det|>label [x0,y0,x1,y1]<|/det|>) followed by its text, with
# coordinates on a 0-999 grid over the input image.
BLOCK_TAG = re.compile(
    r"<\|ref\|>(?P<ref>.*?)<\|/ref\|><\|det\|>(?P<ref_box>.*?)<\|/det\|>"
    r"|<\|det\|>\s*(?P<det>[A-Za-z_][\w-]*)\s*(?P<det_box>\[[^\]]+\])\s*<\|/det\|>",
    re.S,
)
GRID = 999
# Slide chrome that repeats on every slide; kept in the blocks but left out of
# the slide's indexed text. The page number is kept separately as identity.
CHROME_LABELS = {"header", "footer", "page_number", "page_footnote"}
PAGE_NUMBER = re.compile(r"(\d+)\s*/\s*\d+|^\s*(\d+)\s*$")
FIGURE_LABEL = "image"
CAPTION_LABEL = "image_caption"

THUMB_SIZE = (160, 90)
# PDF pages are rendered so their longer side is about this many pixels:
# the model tiles the image anyway, and larger renders only cost memory.
PAGE_LONG_SIDE = 1600
# Figure context stays well inside one text-embedder input.
CONTEXT_CHARS = 1500
# Smaller boxes are logos, icons and slide decoration (the university seal
# sits on every slide at about 0.2% of the frame); real figures measured
# 25-35% on lecture slides and PDF screenshots.
MIN_FIGURE_AREA = 0.01


@attr.s(auto_attribs=True, kw_only=True)
class Thresholds:
    sample_seconds: float = 1.0
    # A thumbnail pixel counts as changed past this grey-level delta, and a
    # sample starts a new segment once this fraction of pixels changed.
    pixel_delta: int = 24
    change_fraction: float = 0.02
    # Shorter segments are transitions, animations or a passing cursor and
    # are folded into the preceding slide.
    min_segment_seconds: float = 3.0
    # Adjacent segments whose words overlap this much are one slide.
    text_overlap: float = 0.85


@attr.s(auto_attribs=True)
class Page:
    """A slide (with its time on screen) or a PDF page, plus what OCR read."""

    image: Image.Image = attr.ib(repr=False)
    start: float | None = None
    end: float | None = None
    thumb: np.ndarray | None = attr.ib(default=None, repr=False)
    blocks: list = attr.ib(factory=list, repr=False)
    text: str = ""
    page_number: int | None = None


def changed(a, b, th):
    return np.mean(np.abs(a - b) > th.pixel_delta) > th.change_fraction


def crop(image, fractions):
    w, h = image.size
    x0, y0, x1, y1 = fractions
    return image.crop((round(x0 * w), round(y0 * h), round(x1 * w), round(y1 * h)))


def video_duration(video_path, fallback):
    with av.open(str(video_path)) as container:
        if container.duration:
            return container.duration / av.time_base
    return fallback


def sample_frames(video_path, th):
    with av.open(str(video_path)) as container:
        stream = container.streams.video[0]
        stream.thread_type = "AUTO"
        next_t = 0.0
        for frame in container.decode(stream):
            if frame.time is None or frame.time < next_t:
                continue
            next_t = frame.time + th.sample_seconds
            image = frame.to_image()
            thumb = image.convert("L").resize(THUMB_SIZE, Image.BILINEAR)
            yield frame.time, np.asarray(thumb, dtype=np.int16), image


def detect_slides(video_path, th):
    """Contiguous segments covering the video, each represented by its last
    sampled frame (the most complete state of a slide that builds up)."""
    segments = []
    current = None
    previous_thumb = None

    def close(segment, end):
        segment.end = end
        if segment.end - segment.start >= th.min_segment_seconds or not segments:
            segments.append(segment)
        else:
            segments[-1].end = end

    for t, thumb, image in sample_frames(video_path, th):
        if current is None:
            current = Page(image=image, start=0.0, end=t, thumb=thumb)
        elif changed(previous_thumb, thumb, th):
            close(current, t)
            current = Page(image=image, start=t, end=t, thumb=thumb)
        else:
            current.thumb, current.image = thumb, image
        previous_thumb = thumb
    if current is not None:
        close(current, video_duration(video_path, current.start))

    merged = segments[:1]
    for segment in segments[1:]:
        if changed(merged[-1].thumb, segment.thumb, th):
            merged.append(segment)
        else:
            segment.start = merged[-1].start
            merged[-1] = segment
    return merged


def render_pdf(pdf_path):
    """Every page of a PDF as an image, numbered from 1."""
    import pymupdf

    pages = []
    with pymupdf.open(pdf_path) as doc:
        for index, page in enumerate(doc):
            zoom = PAGE_LONG_SIDE / max(page.rect.width, page.rect.height, 1)
            pixmap = page.get_pixmap(matrix=pymupdf.Matrix(zoom, zoom), alpha=False)
            image = Image.frombytes("RGB", (pixmap.width, pixmap.height), pixmap.samples)
            pages.append(Page(image=image, page_number=index + 1))
    return pages


def parse_blocks(raw):
    """Grounded OCR output as [{label, boxes, text}], boxes as frame fractions."""
    tags = list(BLOCK_TAG.finditer(raw))
    blocks = []
    lead = raw[: tags[0].start() if tags else len(raw)].strip()
    if lead:
        blocks.append({"label": "text", "boxes": [], "text": lead})
    for tag, following in zip(tags, tags[1:] + [None]):
        try:
            boxes = ast.literal_eval((tag["ref_box"] or tag["det_box"]).strip())
        except (ValueError, SyntaxError):
            boxes = []
        if boxes and isinstance(boxes[0], (int, float)):
            boxes = [boxes]
        blocks.append(
            {
                "label": (tag["ref"] or tag["det"]).strip(),
                "boxes": [[round(v / GRID, 4) for v in b] for b in boxes if len(b) == 4],
                "text": raw[tag.end() : following.start() if following else len(raw)].strip(),
            }
        )
    return blocks


def center(box):
    x0, y0, x1, y1 = box
    return (x0 + x1) / 2, (y0 + y1) / 2


def attach_captions(blocks):
    """Give each figure the caption nearest to it, so a figure crop carries
    the words students will search for it by."""
    figures = [b for b in blocks if b["label"] == FIGURE_LABEL and b["boxes"]]
    for caption in (b for b in blocks if b["label"] == CAPTION_LABEL and b["boxes"]):
        if not figures:
            return
        cx, cy = center(caption["boxes"][0])
        nearest = min(
            figures,
            key=lambda f: min((center(box)[0] - cx) ** 2 + (center(box)[1] - cy) ** 2 for box in f["boxes"]),
        )
        nearest["caption"] = "\n".join(filter(None, [nearest.get("caption"), caption["text"]]))


def page_number(blocks):
    for block in blocks:
        if block["label"] == "page_number" and (match := PAGE_NUMBER.search(block["text"])):
            return int(match[1] or match[2])
    return None


def read_page(page, raw):
    """Fill a page from its raw grounded OCR output."""
    page.blocks = parse_blocks(raw)
    attach_captions(page.blocks)
    page.text = "\n\n".join(b["text"] for b in page.blocks if b["text"] and b["label"] not in CHROME_LABELS)
    page.page_number = page_number(page.blocks) or page.page_number


def words(text):
    return Counter(re.findall(r"\w+", text.lower()))


def contained(a, b):
    """Share of a's words that also occur in b."""
    total = sum(a.values())
    return sum((a & b).values()) / total if total else 0.0


def merge_same_slide(pages, overlap):
    """Fold adjacent segments that show the same slide: a bullet build, a cut
    between full-screen and the studio composite, or a presenter gesture all
    leave the words largely contained in each other while OCR noise keeps
    them from matching exactly. When the model read a page number on both,
    that decides instead. The wordier frame represents the merge."""
    merged = pages[:1]
    for page in pages[1:]:
        previous = merged[-1]
        a, b = words(previous.text), words(page.text)
        if previous.page_number is not None and page.page_number is not None:
            same = previous.page_number == page.page_number
        else:
            same = (not a and not b) or max(contained(a, b), contained(b, a)) >= overlap
        if not same:
            merged.append(page)
            continue
        keep = page if sum(b.values()) >= sum(a.values()) else previous
        keep.start, keep.end = previous.start, page.end
        merged[-1] = keep
    return merged


def title(blocks):
    for block in blocks:
        if block["label"].endswith("title") and block["text"]:
            return block["text"].lstrip("#").strip()
    return ""


@attr.s(auto_attribs=True)
class Figure:
    page_position: int
    bbox: list
    image: Image.Image = attr.ib(repr=False)
    caption: str | None
    context: str


def figures_on(position, page):
    """Crop every figure block of a page, with the text students would
    describe it by: the slide title, its caption, and the slide's text."""
    found = []
    heading = title(page.blocks)
    # The slide text usually opens with the title already.
    if heading and heading in page.text:
        heading = ""
    for block in page.blocks:
        if block["label"] != FIGURE_LABEL:
            continue
        caption = block.get("caption")
        context = "\n\n".join(filter(None, [heading, caption, page.text]))[:CONTEXT_CHARS]
        for fractions in block["boxes"]:
            x0, y0, x1, y1 = fractions
            if (x1 - x0) * (y1 - y0) < MIN_FIGURE_AREA:
                continue
            found.append(
                Figure(
                    page_position=position,
                    bbox=list(fractions),
                    image=crop(page.image, fractions),
                    caption=caption,
                    context=context,
                )
            )
    return found
