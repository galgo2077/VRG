# FASE_4.md — Pruebas autorizadas, corrección y recuperación de fallos

## Inicio y autorización

Activar exclusivamente después de que fase 3 termine la implementación. Usar `plantillas/REGISTRO_PRUEBAS_CERRADO.md` convertido en lista **cerrada y aprobada por el usuario antes de fase 3**. Sin pruebas autorizadas, no ejecutar ninguna; declarar `CERTIFICACION_BLOQUEADA_POR_APROBACION`.

## Objetivo

Verificar funcionalidad mediante el conjunto **mínimo y únicamente aprobado**, reparar fallos reales y dejar una política específica por cada módulo. Ejecutar las pruebas al final del desarrollo, no durante la construcción. No se permite crear tests extra por iniciativa del agente.

## Ejecución de pruebas

1. Congelar revisión de código candidata y referencia exacta del registro cerrado.
2. Consolidar los IDs autorizados de fase 4, su comando exacto y datos autorizados; no añadir pruebas.
3. **Un único runner** ejecuta solo los IDs una vez, agrupando comandos compatibles cuando esté especificado.
4. Clasificar fallos desde resultados resumidos. Agentes paralelos analizan fallos por componentes independientes **sin volver a lanzar suites**.
5. Corregir únicamente módulos afectados, sin relajar seguridad ni requisitos.
6. Repetir solo los tests autorizados relacionados con las correcciones; registrar número de repeticiones y justificación.
7. Si aparece un fallo no cubierto por la lista autorizada, documentar riesgo, corregir si el comportamiento esperado está especificado, pero **no ejecutar una nueva prueba sin permiso**.
8. Emitir informe final con resultados por ID, defectos resueltos, bloqueos y limitaciones; no pegar logs íntegros.

## Matriz de tratamiento de errores requerida

Para cada módulo existente (no asumir que se implementaron todos los candidatos), registrar:

| Área | Fallo ejemplo | Respuesta fail-closed | Recuperación cuando sea segura |
|---|---|---|---|
| Lectura vault | archivo inaccesible | no inventar contenido; estado explícito | reintentar lectura acotada sin alterar original |
| Parseo Markdown | frontmatter malformado | aislar nota/fracción, no corromper | conservar fuente; reparar/parser tolerante si aprobado |
| Enlaces | alias ambiguo o destino faltante | relación no resuelta etiquetada | reindexar resolución al cambiar metadatos |
| Grafo | arista inconsistente | no publicar snapshot incompleto | reconstruir índice derivado desde fuentes |
| Búsqueda textual | índice corrupto | no dar resultados obsoletos como vigentes | reconstrucción controlada de índice |
| Embeddings | modelo ausente/timeout | advertir degradación; no fingir semántica | activar búsqueda exacta solo si política lo permite |
| Fusión RAG | fragmentos duplicados/no atribuibles | excluir respuesta no atribuible | rehacer selección desde fuentes |
| Indexación | interrupción o cambio concurrente | checkpoint, no avanzar manifest sin commit | reanudación idempotente |
| Almacenamiento | falta de espacio/commit incompleto | transacción abortada, original a salvo | rollback/reanudación desde journal |
| Escritura de nota | revision conflict/permisos | no sobrescribir | intervención/aplicación con revisión válida |
| MCP Codex | timeout/mensaje inválido | devolver error estructurado sin fuga | reconectar con límites |
| Conector ChatGPT | bridge no disponible | declarar `NO_CONECTADO`, sin publicar datos | activación autorizada y compatible |
| Sincronización | versiones divergentes | no elegir ganador silenciosamente | reconciliación con historial |
| Seguridad | ruta fuera de vault/secret | denegar acceso | corregir configuración autorizada |

No ejecutar escenarios particulares de esta tabla salvo que haya un ID de prueba aprobado. La tabla es una **especificación de manejo de fallos**, no una autorización de tests.

## Entregables

- `RESULTADOS_CERTIFICACION.md`, por ID aprobado: ejecutado/no ejecutado, resultado, evidencia resumida y cambios correctivos.
- `MATRIZ_RECUPERACION_FINAL.md`, por módulo real: error, señal, efecto, fallback, retry bounded, rollback, severidad.
- Correcciones de código autorizadas y referencias a cambios.
- Lista final de bloqueos reales, incluyendo ChatGPT si no está disponible.

## Condición de salida

Solo afirmar `FASE_4_CERTIFICADA` si TODOS los IDs autorizados de fase 4 pasan y no quedan fallos críticos conocidos que contradigan la especificación. Si no, finalizar `FASE_4_PARCIAL`/`BLOQUEADA` con evidencias y NO iniciar migración de producción.
