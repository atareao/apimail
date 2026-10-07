# Design

## Context

Ver `proposal.md` — Why. Estado de partida (v0.2.0 conceptual): `Config` solo
modela el servidor HTTP (`host`, `port`, `api_key`), `AppState` guarda el digest
de la API key y todas las rutas salvo `GET /api/health` están protegidas por el
middleware `require_api_key`. La carga de configuración es **testeable sin mutar
el entorno** gracias a `Config::from_lookup(F)`, donde `F: Fn(&str) -> Option<String>`.

`mail-account` es **solo configuración y validación**: no abre ninguna conexión de
red (eso llega con `imap-connection` y `smtp-send`).

## Goals / Non-Goals

**Goals:**
- Modelar la cuenta de correo como dos endpoints (IMAP y SMTP) con host, puerto,
  modo TLS, usuario y secreto.
- Validación *fail-closed* al arranque con errores descriptivos.
- Reutilizar `from_lookup` para mantener los tests sin variables de entorno.
- No filtrar secretos (logs / `Debug`).
- Un endpoint protegido (`GET /api/account`) que haga observable la configuración
  no sensible.

**Non-Goals:**
- Abrir conexiones IMAP/SMTP o autenticar contra el servidor.
- Múltiples cuentas, OAuth/OIDC, rotación de secretos, almacén de claves.
- Soporte de certificados personalizados / pinning (se abordará al conectar).

## Decisions

- **Nombres de variables por endpoint** — `APIMAIL_IMAP_*` y `APIMAIL_SMTP_*`,
  cada una con `HOST`, `PORT`, `TLS`, `USER`, `PASSWORD`. Predecible y amplía el
  prefijo `APIMAIL_` ya existente.
  *Alternativas descartadas:* una sola `APIMAIL_MAIL_URL` (difícil de validar y
  propensa a errores de escapado); compartir usuario/clave entre IMAP y SMTP
  (los proveedores no siempre coinciden).
- ***Fail-closed*: host, usuario y secreto obligatorios y no vacíos** en ambos
  endpoints. El servicio sin cuenta de correo no sirve para nada; un arranque
  silenciosamente incompleto solo aplaza el fallo. Requerir IMAP **y** SMTP ya
  ahora es coherente con que el roadmap los necesita a ambos y evita una segunda
  ruptura de arranque más adelante.
- **`TlsMode { Implicit, StartTls, None }`**, por defecto `Implicit`, parseado sin
  distinguir mayúsculas. `None` se permite (con *warning*) porque los servidores
  de prueba locales (MailHog/GreenMail) no ofrecen TLS; prohibirlo dificultaría el
  desarrollo. El aviso deja claro que las credenciales irían en claro.
- **Puerto por defecto derivado del TLS** — IMAP `993`/`143`/`143` y SMTP
  `465`/`587`/`587`. Evita el error clásico de `starttls` sobre el puerto implícito
  (993/465) y hace que un `PORT` explícito sea una excepción, no la norma.
  *Alternativa descartada:* fijar `993`/`465` siempre y obligar a declarar el
  puerto para otros modos (más fácil de equivocar).
- **Tipos dedicados** en `src/config.rs` (junto al resto de la configuración, que
  es su naturaleza): `MailEndpoint { host, port, tls, username, password }`,
  `MailAccount { imap, smtp }` y `AccountError` con `thiserror`
  (`MissingValue`, `InvalidPort`, `InvalidTls`). `ConfigError` gana una variante
  `Account(#[from] AccountError)`.
  *Alternativa descartada:* un módulo `account.rs` separado (el roadmap reserva
  `imap`/`smtp` para la lógica de conexión, no para la config).
- **`Config::from_lookup` amplía su contrato**: tras host → port → api key, carga
  `MailAccount::from_lookup(&lookup)`. El orden mantiene que un `APIMAIL_PORT` de
  servidor inválido se siga reportando primero.
- **Vista pública en `AppState`** — el estado guarda únicamente
  `AccountView { imap: EndpointView { host, port }, smtp: EndpointView { host, port } }`
  (sin usuario ni secreto) para el handler `GET /api/account`. Así el secreto no
  se propaga por el estado compartido; cuando `imap-connection` necesite las
  credenciales, añadirá lo que requiera en su propio change.
  *Alternativa descartada:* meter el `MailAccount` completo en `AppState`
  (reparte el secreto por más sitios sin necesidad todavía).
- **`GET /api/account`** se monta en el **sub-router protegido**, reutilizando el
  middleware existente; su cuerpo
  `{"imap":{"host":...,"port":...},"smtp":{"host":...,"port":...}}` no incluye
  usuario, TLS ni secretos.
- **Redacción**: `Debug` manual en `MailEndpoint` (y por tanto en `MailAccount`)
  sustituye el secreto por `***`, igual que ya se hace con la API key.

## Risks / Trade-offs

- [Exigir IMAP y SMTP a la vez puede molestar a quien solo uses IMAP] → Es el
  menor de los males: evita un segundo cambio de arranque y refleja la cuenta
  real; las credenciales SMTP se pueden repetir de las IMAP si el proveedor las
  comparte.
- [Permitir TLS `none` habilita credenciales en claro] → Solo se acepta con
  *warning* explícito; el caso de uso es pruebas locales. El endurecimiento
  (rechazo en producción) puede venir con un flag futuro.
- [Derivar el puerto del TLS es "mágico"] → Se documenta en el README y en el
  propio error de ayuda; un `PORT` explícito siempre gana.
- [`Config` crece y su `from_lookup` se alarga] → Se mantiene lineal y con tests
  por caso; si crece más, se extraerá un submódulo de carga.

## Migration Plan

No hay despliegue en producción. Quien ejecute `cargo run` en local deberá
declarar las seis variables obligatorias (host/usuario/secreto de IMAP y SMTP),
además de `APIMAIL_API_KEY`. El `README` se actualizará con la tabla de variables
y un ejemplo. La actualización de `AGENTS.md`/`README` puede ir en este mismo
change (documentación) o en uno posterior.
