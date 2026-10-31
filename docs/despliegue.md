# Despliegue en producción — referencia literal

## Despliegue en producción (verificado el 2026-10-10)

Servicio en vivo: **https://apimail.territoriolinux.es** (host `co1`, detrás de Traefik con TLS).
Verificación contra producción: **20 endpoints** ejercitados (lectura y escritura, incluido
`POST /api/messages` con entrega real a un buzón externo); el artefacto desplegado es byte a byte
el publicado por CI (`id=sha256:8b3a550ce6ab…`, `digest=sha256:12e895cdd260…`).

- **Red**: el contenedor `apimail` debe estar en la red externa `proxy`, compartida con Traefik
  (`networks: [- proxy]` en el servicio + `networks: { proxy: { external: true } }` a nivel raíz).
  Sin ella, Traefik no resuelve el nombre `apimail` y la URL pública devuelve **504**. La red
  `apimail_default` queda huérfana y puede eliminarse.
- **Stack**: gestionado por **Dockge**; el compose vive en
  `co1:/home/lorenzo/docker/dockge/stacks/apimail/compose.yaml` (montado como `/opt/stacks`).
  Los secretos y la `APIMAIL_API_KEY` viven en el `.env` de ese directorio.
- **IDLE/webhook desactivado a propósito**: `APIMAIL_WEBHOOK_URL` está **vacío** en producción, así
  que `GET /api/idle/status` responde `stopped` y `POST /api/idle/start` devuelve **501
  `idle_not_configured`**. No es un fallo.
- **Aviso de infraestructura (no es apimail)**: el entrypoint `https` de Traefik aplica
  `shuul-auth@file` (un `forwardAuth` a `http://shuul:3000/api/v1/shuul`) a **todo** el tráfico.
  Devuelve **403 `Ko`** a las peticiones cuya IP origen es la pública de `co1`; por eso probar
  desde `co1` contra la URL pública da 403 y hay que usar la red interna del contenedor.
