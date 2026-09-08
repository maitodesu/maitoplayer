export interface PopoverAnchorRect {
  left: number;
  top: number;
  bottom: number;
  width: number;
}

export interface PopoverLayoutInput {
  viewportWidth: number;
  viewportHeight: number;
  anchor: PopoverAnchorRect;
  desiredHeight: number;
}

export interface PopoverLayout {
  left: number;
  top: number;
  width: number;
  maxHeight: number;
  placement: 'above' | 'below';
}

const VIEWPORT_MARGIN = 16;
const ANCHOR_GAP = 12;
const MAX_WIDTH = 480;
const MAX_HEIGHT = 448;

export function calculatePopoverLayout({
  viewportWidth,
  viewportHeight,
  anchor,
  desiredHeight,
}: PopoverLayoutInput): PopoverLayout {
  const availableWidth = Math.max(0, viewportWidth - VIEWPORT_MARGIN * 2);
  const width = Math.min(MAX_WIDTH, availableWidth);
  const viewportHeightLimit = Math.max(0, viewportHeight - VIEWPORT_MARGIN * 2);
  const boundedDesiredHeight = Math.min(
    MAX_HEIGHT,
    viewportHeightLimit,
    Math.max(0, desiredHeight),
  );
  const roomAbove = Math.max(0, anchor.top - VIEWPORT_MARGIN - ANCHOR_GAP);
  const roomBelow = Math.max(0, viewportHeight - anchor.bottom - VIEWPORT_MARGIN - ANCHOR_GAP);
  const placement = roomAbove >= boundedDesiredHeight || roomAbove > roomBelow ? 'above' : 'below';
  const placementRoom = placement === 'above' ? roomAbove : roomBelow;
  const maxHeight = Math.min(boundedDesiredHeight, placementRoom);
  const displayedHeight = Math.min(boundedDesiredHeight, maxHeight);
  const centeredLeft = anchor.left + anchor.width / 2 - width / 2;
  const left = Math.min(
    Math.max(VIEWPORT_MARGIN, viewportWidth - width - VIEWPORT_MARGIN),
    Math.max(VIEWPORT_MARGIN, centeredLeft),
  );
  const top =
    placement === 'above'
      ? Math.max(VIEWPORT_MARGIN, anchor.top - ANCHOR_GAP - displayedHeight)
      : Math.min(viewportHeight - displayedHeight - VIEWPORT_MARGIN, anchor.bottom + ANCHOR_GAP);

  return { left, top, width, maxHeight, placement };
}
