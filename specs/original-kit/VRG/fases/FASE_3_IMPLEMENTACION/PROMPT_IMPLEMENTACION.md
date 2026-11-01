# PROMPT_IMPLEMENTACION.md — Ejecutar solo fase 3

Eres coordinador de implementación Rust de VRG. Lee los documentos de esta fase, la auditoría validada, el plan de arquitectura y la lista cerrada de pruebas **APROBADA**. Si la lista no está aprobada, no iniciar implementación que derive en validación no autorizada: declarar bloqueo y explicar decisión faltante.

## Plan de ejecución

1. Confirma la base aislada del Jardinero y el conjunto exacto de módulos aprobados.
2. Define APIs y tipos comunes, fronteras de módulos, esquema de datos, fuentes, seguridad, políticas de errores y ownership. No crear crates redundantes.
3. Reparte 8–12+ trabajadores en módulos **disjuntos** si hay suficiente trabajo y recursos. Inicia frentes independientes a la vez; tareas dependientes solo cuando sus contratos estén definidos.
4. Implementa todo el código, conexión MCP local, recuperación híbrida, grafo y persistencia estipulados, sin correr pruebas durante la construcción. Cada agente publica cambios mínimos y contratos.
5. Coordina la integración de interfaces compartidas; usa revisión estática de cambios, no ejecuciones de prueba.
6. Completa toda la implementación autorizada y luego ejecuta una **única validación final agrupada**: `cargo check --workspace` y handshake mínimo Codex↔VRG. Si cargo check revela errores, corrige y vuelve a ejecutarlo solo cuando sea indispensable para cerrar el fallo; esto no autoriza suites de pruebas.
7. Para ChatGPT, solo confirma handshake cuando existe conector remoto/túnel expresamente autorizado y la cuenta lo permite. Si no, entrega interfaz pendiente, no anuncies éxito.
8. Entrega contratos, matriz de fallos de diseño, estado preciso y un informe final completo; progreso de chat ≤3 líneas.

## Regla de oro para ahorrar tokens

No releer el vault entero; no imprimir logs; no hacer mini-tests; no ejecutar tests por módulo; no perseguir lint/format sin permiso; no investigar opciones tecnológicas que ya estén aprobadas y no cambien; no generar múltiples versiones de documentación con la misma información.

## Seguridad

No modificar notes reales en fase 3, no conectar una instancia remota que filtre datos privados, no exponer claves, no colocar embeddings afuera sin aprobación. Usar fixtures o mocks únicamente como datos estáticos de implementación si están autorizados, pero no ejecutarlos como pruebas.

## Cierre

`CODIGO_INTEGRADO_COMPILA` si cargo check termina; `MCP_HANDSHAKE_CONFIRMADO` solo con evidencia de inicialización y descubrimiento; `CHATGPT_PENDIENTE` si procede; `FUNCIONALIDAD_NO_PROBADA` siempre hasta fase 4. Detenerse sin ejecutar fase 4.
