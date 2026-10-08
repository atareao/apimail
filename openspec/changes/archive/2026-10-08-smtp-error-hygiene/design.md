# Design

## Context

`SmtpError` (`src/smtp.rs`) tiene tres variantes:

```rust
pub enum SmtpError {
    Message(#[from] MessageError),          // validación/conversión del mensaje
    Transport(#[from] lettre::transport::smtp::Error),
    Delivery(String),                       // rechazo/fallo del servidor
}
```

Su `Display` sí explica el error, pero para `Transport` y `Delivery` **interpola
texto de terceros** (de `lettre`, rustls o el propio servidor SMTP). Hoy
`send_message` (`src/http.rs:1297`) hace `SendError::Smtp(error.to_string())`, de
modo que ese texto viaja al cliente en el `502`. El handler IMAP equivalente ya
lo evita: mapea a `ImapError::public_message()` y reserva `kind()` para logs
(`src/imap.rs`, ~80–120). Este change replica ese patrón en SMTP.

## Goals / Non-Goals

**Goals:**
- Que el body del `502` sea **estable, independiente del servidor y sin texto de
  terceros**, coherente con el resto de la API.
- Añadir `SmtpError::public_message()` (`&'static str`) para HTTP y
  `SmtpError::kind()` (`&'static str`) para logs.
- Cubrir con tests unitarios que los mensajes/etiquetas no filtran el texto.
- Reforzar `smtp_failure_is_502` para que deje de enmascarar la fuga.

**Non-Goals:**
- **No** tocar el `serde_json` de un body malformado (`SendError::InvalidRequest`):
  es texto estructural del parser (nombres de campo, línea/columna), no texto de
  terceros, y no se considera fuga.
- **No** cambiar el status (`502`) ni el código (`smtp_error`) del contrato HTTP.
- **No** alterar la validación de mensajes (`MessageError`) ni sus mensajes de
  `400`, que ya son propios y estables.
- **No** añadir logging nuevo ni dependencias.

## Decisions

- **`SmtpError::public_message(&self) -> &'static str`**: mensaje externo por
  variante, agrupando por familia para que sea estable entre versiones de
  `lettre`:
  - `Message(_)` → `"the message could not be validated"`
  - `Transport(_)` → `"failed to create the SMTP transport"`
  - `Delivery(_)` → `"SMTP delivery failed"`

  Alternativa descartada: partir de `self.to_string()` y recortar; el `Display`
  ya arrastra el texto de terceros y no hay forma fiable de separarlo.

- **`SmtpError::kind(&self) -> &'static str`**: etiqueta compacta para tracing
  (`"message"`, `"transport"`, `"delivery"`), igual que `ImapError::kind`. Es
  estática y machine-filterable, sin datos del servidor.

- **Uso en `send_message`**: sustituir

  ```rust
  .map_err(|error| SendError::Smtp(error.to_string()))?;
  ```

  por

  ```rust
  .map_err(|error| SendError::Smtp(error.public_message().to_string()))?;
  ```

  `SendError::Smtp(String)` no cambia de forma; solo cambia su contenido.

- **El detalle vive solo fuera de HTTP**: el `Display` de `SmtpError` (con el
  texto crudo) se conserva para diagnóstico interno y para los tests, pero no se
  usa al construir respuestas HTTP ni se registra por defecto en el camino de
  envío. Nunca aparece en el body.

- **Trade-off — no loguear el texto crudo**: renunciamos a registrar el
  `Display` completo del fallo SMTP, porque ese texto puede incluir banners,
  nombres de host, direcciones o detalles de certificado (la misma clase de dato
  que queremos proteger). A cambio perdemos algo de detalle para depurar; se
  compensa con `kind()` como etiqueta filtrable y con el `Display` disponible
  si en el futuro se decide loguearlo con redacción explícita.

- **Fake de test con centinela**: `FakeMailer::failing` devuelve
  `SmtpError::Delivery("smtp.internal.example said: 550 rejected")` (host +
  banner del servidor). El test se refuerza para afirmar que el `message` del
  `502` es exactamente `"SMTP delivery failed"` y que **no** contiene el centinela
  completo ni sus fragmentos (el host `smtp.internal.example` ni el banner
  `550 rejected`). Así el contrato estabilizado queda blindado.

## Risks / Trade-offs

- [Menos detalle en el `502` para el cliente] → es el objetivo: el cliente
  necesita saber que fue un fallo de entrega, no la topología interna del relay.
- [Menos detalle crudo en logs] → mitigado por `kind()` y por el `Display` que
  sigue existiendo internamente; documentado arriba.
- [El mapeo puede quedarse corto si `lettre` añade variantes] → `public_message`
  y `kind` son `match` exhaustivos sobre nuestras tres variantes, que son
  estables; una variante futura forzará a actualizar el mapeo en compilación.

## Migration Plan

No hay despliegue en producción ni migración de datos. El contrato HTTP se
mantiene (mismo status y código); solo se estabiliza el texto del `message`.
