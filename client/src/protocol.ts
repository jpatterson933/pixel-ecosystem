export type Rgba = [number, number, number, number]

export interface WorldConfig {
  map_width: number
  map_height: number
  pixel_count: number
  color_count: number
  tick_rate: number
  seed: number
}

export interface WorldInfo {
  world_id: string
  config: WorldConfig
  summary_levels: number
}

export interface PixelState {
  id: number
  x: number
  y: number
  color: Rgba
}

export interface CellState {
  x: number
  y: number
  color: Rgba
}

export type PixelEvent =
  | { t: 'placed'; id: number; x: number; y: number; color: Rgba }
  | { t: 'arrived'; id: number; x: number; y: number; color: Rgba }
  | { t: 'moved'; id: number; from: [number, number]; to: [number, number] }
  | { t: 'departed'; id: number; from: [number, number]; to: [number, number] }
  | { t: 'cell_set'; x: number; y: number; color: Rgba | null }
  | { t: 'attribute_set'; id: number; module: string; key: string; value: unknown }
  | { t: 'config'; config: WorldConfig }

export interface WorldMessage extends WorldInfo {
  type: 'world'
  tick: number
}

export interface ChunkMessage {
  type: 'chunk'
  tick: number
  cx: number
  cy: number
  pixels: PixelState[]
  cells?: CellState[]
}

export interface EventsMessage {
  type: 'events'
  tick: number
  cx: number
  cy: number
  events: PixelEvent[]
}

export type ServerMessage = WorldMessage | ChunkMessage | EventsMessage

export type ChunkCoord = [number, number]

export interface SubscribeMessage {
  type: 'subscribe'
  chunks: ChunkCoord[]
}

export type ClientMessage = SubscribeMessage
