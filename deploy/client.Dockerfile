FROM node:24-alpine AS build
WORKDIR /app
COPY client/package*.json ./
RUN npm ci
COPY client ./
ARG VITE_GATEWAY_URL=http://localhost:8080
ARG VITE_TILE_BASE_URL=http://localhost:8082
ENV VITE_GATEWAY_URL=$VITE_GATEWAY_URL VITE_TILE_BASE_URL=$VITE_TILE_BASE_URL
RUN npm run build

FROM nginx:alpine
COPY --from=build /app/dist /usr/share/nginx/html
