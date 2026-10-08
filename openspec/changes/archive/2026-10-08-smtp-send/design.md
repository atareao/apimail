# Design

## Context

Ver `proposal.md` — Why. Estado de partida: `mail-account` ya carga y valida el
endpoint SMTP (`MailEndpoint { host, port, tls, username, password }`) en
`Config.account.smtp`; `AppState::from_config(&Config)` construye el estado y
`build_router(state)` monta el sub-router protegido por `require_api_key`. La
configuración se testea sin mutar el entorno vía `from_lookup`.

`smtp-send` **no depende de IMAP**; solo necesita la cuenta SMTP y el runtime
async que ya existe (`tokio`).

## Goals / Non-Goals

**Goals:**
- Enviar correo saliente (texto, HTML y adjuntos) por SMTP con `lettre`.
- Respetar el modo TLS de `mail-account` con **rustls**.
- Poder testear el endpoint **sin red**.
- Acotar el tamaño de los adjuntos y traducir los fallos SMTP a HTTP.
- No filtrar credenciales.

**Non-Goals:**
- Colas, entrega asíncrona, reintentos con backoff o programación de envíos.
- DKIM, plantillas, tracking de apertura, `Reply-To`/cabeceras arbitrarias.
- Verificar la conexión SMTP al arrancar (no se abre red en startup).
- Múltiples cuentas.

## Decisions

- **`lettre` con rustls, sin `native-tls`**: `default-features = false,
  features = ["builder", "smtp-transport", "tokio1-rustls-tls", "hostname"]`.
  Coherente con la decisión transversal del `PLAN.md` (rustls) y evita depender
  de OpenSSL. `base64` se añade para decodificar los adjuntos del JSON.
- **Capa de transporte inyectable**: `trait MailSender: Send + Sync` con un
  único método `send(&self, message: OutgoingMessage)` que devuelve un **futuro
  boxeado** (`Pin<Box<dyn Future<Output = Result<(), SendError>> + Send + '_>>`).
  Se evita `async-trait` (una dependencia más) y se mantiene `Arc<dyn MailSender>`
  *dyn-compatible*. `AppState` guarda `Arc<dyn MailSender>`, de modo que los tests
  inyectan un **emisor falso** y no tocan la red; el binario construye
  `SmtpSender`.
- **`OutgoingMessage`** (dominio) desacoplado del DTO HTTP: `from`, `to`, `cc`,
  `bcc`, `subject`, `text`, `html` y `attachments: Vec<OutgoingAttachment>`. El
  handler valida y convierte el DTO; el transporte solo habla el dominio.
- **Transporte construido una vez al arrancar** con
  `AsyncSmtpTransport::<Tokio1Executor>`:
  - `implicit` → `relay(host)` (TLS envolvente).
  - `starttls` → `starttls_relay(host)`.
  - `none` → `builder_dangerous(host).tls(Tls::None)`.
  La elección se aísla en la función pura `tls_strategy(TlsMode) -> TlsStrategy`
  para poder testearla sin red.
  Luego `.port(port).credentials(Credentials::new(user, pass)).build()`.
  *No resuelve DNS ni conecta al construir*; la conexión ocurre en `send`.
  Consecuencia: `AppState::from_config` pasa a devolver `Result` (construir el
  transporte puede fallar) y `main.rs` lo trata como fallo de arranque.
- **Payload JSON + adjuntos en base64**. Formato: `{from?, to[], cc[]?, bcc[]?,
  subject?, text?, html?, attachments[]?}` con cada adjunto
  `{filename, content_type?, data_base64}`. Es consistente con el resto de la API
  JSON y evita el parser `multipart`.
- **`from` opcional**; si falta se usa `APIMAIL_SMTP_USER`. Si el valor efectivo
  no es un *mailbox* válido → `400`.
- **Validación de direcciones con `lettre`** (`Mailbox`), no con una regex
  casera: acepta `Nombre <user@host>` y direcciones simples.
- **Límite de tamaño** con `APIMAIL_MAX_ATTACHMENT_BYTES` (default 10 MiB)
  en `Config.max_attachment_bytes`, validado *fail-closed* (positivo; si no,
  error de arranque que nombra la variable). El `DefaultBodyLimit` de axum se
  **deshabilita** en la ruta y el handler lee el cuerpo con
  `axum::body::to_bytes(body, message_body_limit())`, de modo que un cuerpo
  sobredimensionado se responde como **`413` con el envelope JSON** (y no con el
  `413` en texto plano de axum). `message_body_limit()` = `límite * 4/3` + 64 KiB
  de holgura para el resto del JSON (aritmética saturada). Además se comprueba la
  suma de bytes **decodificados** contra el límite (también → `413`).
  *Alternativa descartada:* `DefaultBodyLimit::max(...)` (su rechazo no sigue el
  contrato de errores JSON) o no acotar (riesgo de agotar memoria).
- **Contrato de errores**: `400` (JSON inválido o campos inválidos), `413`
  (tamaño excedido), `502` (fallo del servidor SMTP), `401` (sin API key). Los
  cuerpos siguen el patrón `{"error":...,"message":...}` ya usado por `401`.
- **Redacción**: `SmtpSender` guarda el transporte de `lettre` (con credenciales);
  el `Debug` de `AppState` **no** lo imprime (se muestra como `***`), para no
  filtrar usuario/contraseña.

## Risks / Trade-offs

- [Los tests no envían correo real] → Cubierto con el emisor falso inyectado; el
  camino `lettre` (mapeo de mensaje y errores) se prueba con unitarios del
  `OutgoingMessage`/DTO y con un test que fuerza un error del emisor falso para
  verificar el `502`. La integración real queda como verificación manual
  (documentada) o en un futuro `imap-idle`.
- [Construir el transporte al arrancar puede fallar y romper el arranque] →
  Es deseable: una config SMTP inutilizable debe fallar pronto y con mensaje
  claro (fail-closed), no en el primer envío.
- [base64 + JSON cargan el mensaje entero en memoria] → Aceptable con un límite
  explícito y por defecto moderado (10 MiB); el streaming de adjuntos queda como
  mejora futura.
- [`AppState::from_config` cambia de firma] → Es API interna; obliga a tocar los
  tests existentes (`.expect(...)`), lo cual se hace dentro de este change (RED
  primero).
- [Desviación `413` frente al `400` aprobado] → Se documenta y se confirma en la
  revisión de la spec; `413` es el código correcto para "demasiado grande".

## Migration Plan

No hay despliegue en producción. Quien quiera acotar los adjuntos puede fijar
`APIMAIL_MAX_ATTACHMENT_BYTES`; por defecto son 10 MiB. El `README` se actualizará
con el endpoint, el ejemplo de `curl` y la nueva variable.
