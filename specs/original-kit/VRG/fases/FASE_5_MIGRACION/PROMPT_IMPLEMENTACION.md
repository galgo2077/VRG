# PROMPT_IMPLEMENTACION.md — Ejecutar solo fase 5

Actúa como coordinador de migración final VRG. Debes preservar el vault visible de Obsidian como **fuente** y crear índices RAG/grafo **derivados**. Lee la especificación, `FASE_5.md` y `GOAL.md`, resultados de certificación, manifiestos y lista cerrada de comprobaciones fase 5.

## Procedimiento

1. Comprueba que la fase 4 fue certificada y que el conjunto de vaults, exclusiones, backup, journal, autorizaciones de escritura y comprobaciones posmigración fue aprobado.
2. Si falta una autorización crítica, no migrar de manera parcial encubierta: declarar bloqueo puntual.
3. Realiza un inventario de notas autorizadas con IDs y revisiones sin modificar archivos originales.
4. Procesa lotes independientes en paralelo (~8–12+ cuando hay datos/recursos), con caches por shard y commits centralizados.
5. Construye índice textual, semántico y grafo en sombra, siempre con rutas y referencias al Markdown.
6. Trata los fallos por nota como estados trazables; trata errores de transacción como abort/rollback, nunca como éxito parcial silencioso.
7. Publica snapshots derivados únicamente mediante commit/journal atómico y política de fallback.
8. Ejecuta **solo** las comprobaciones de migración que ya aparecen como pruebas aprobadas de fase 5 y registra resultado por ID. No diseñar otras ni ejecutar suites de regresión.
9. Publica informe final detallado: total autorizado, indexado, excluido, fallido, enlaces resueltos/ambiguos, revisiones, rollback y pendientes. No imprimir logs íntegros.
10. Detente después del cierre de esta fase. No iniciar migración física, entrenamiento adicional ni proyectos laterales.

## Seguridad

- Nunca borrar/modificar originales al generar índice; las operaciones de escritura a notas solo existen como funcionalidad aprobada y con revisión + historial.
- Sin uploads externos ni ChatGPT remoto salvo autorización técnica y de privacidad explícita.
- Datos sensibles excluidos. No indexar carpetas fuera de raíces autorizadas, ni seguir symlinks de salida.
- La ausencia de pruebas finales aprobadas obliga a etiquetar `MIGRADA_SIN_VALIDACION_AUTORIZADA`, no a improvisar pruebas.

## Condición de parada

Declarar `FASE_5_COMPLETA_Y_VALIDADA` solo con cobertura autorizada y comprobaciones aprobadas superadas; `FASE_5_PARCIAL`/`BLOQUEADA` si no. El cierre no autoriza futuros syncs masivos que alteren notas sin nuevo alcance.
