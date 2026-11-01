# Protocolo multiagente — ejecución paralela real

## Política de concurrencia

- Descomponer el DAG por tareas y dependencias reales, no por fases artificiales dentro de la misma fase.
- Meta inicial: **8 a 12 agentes útiles simultáneos**; pueden ser más si hay particiones independientes y memoria/CPU suficientes. La ausencia de límite fijo no significa crear agentes ilimitados.
- Trabajos independientes se despachan juntos. Solo integración, archivos compartidos o permisos causan dependencia.
- Evitar 8 instancias redundantes leyendo el mismo árbol: asignar subárboles, contratos y resultados distintos.
- Agentes trabajadores no redefinen GOAL ni pruebas y no arrancan fase siguiente.

## Roles sugeridos (no presupone archivos reales)

| Rol | Fase 1 | Fase 2 | Fase 3 | Fase 4 | Fase 5 |
|---|---|---|---|---|---|
| A01 | Topología y repositorios | Superficie antigua A | Contratos/API core | Diagnóstico core | Ingestión partición A |
| A02 | Vault y Markdown | Superficie antigua B | Parseo/IDs/links | Diagnóstico parseo | Ingestión B |
| A03 | Scripts/hooks | Superficie antigua C | Graph store | Diagnóstico grafo | Ingestión C |
| A04 | Sincronización | Superficie antigua D | Recuperación textual | Diagnóstico búsqueda | Ingestión D |
| A05 | MCP/Codex | Surface antigua E | Motor semántico | Diagnóstico MCP | Ingestión E |
| A06 | Seguridad y secretos | Surface antigua F | Indexación incremental | Diagnóstico persistencia | Ingestión F |
| A07 | Historial/errores | Surface antigua G | Journal, rollback, permisos | Diagnóstico seguridad | Ingestión G |
| A08 | Dependencias y rendimiento estático | Surface antigua H | MCP local / CLI | Diagnóstico journal | Ingestión H |
| A09 | Grafo real / referencias | Solo si hay frente separado | Integración con Obsidian | Diagnóstico integración | Resolver enlaces |
| A10 | Contratos externos | Solo si hay frente separado | Adaptador ChatGPT condicionado | Análisis de fallos | Reporte/cutover |
| Coordinador | Matriz de evidencias | Aplica matriz aprobada | Unifica interfaces y merge | **Único runner de tests aprobados** | Único responsable de commit/cutover |

Asignar A01–A10 solo después de identificar ámbitos reales. Los nombres de roles son hipótesis organizativas, no evidencia del código.

## Protección de escrituras

1. Cada tarea recibe `ID`, dueño, rutas de solo lectura, rutas editables exclusivas, entradas, entregables y stop condition.
2. El coordinador asigna interfaces con contratos congelados antes de cambios de implementación que dependan de ellos.
3. Prohibida edición simultánea del mismo archivo por agentes distintos.
4. Si dos agentes necesitan el mismo archivo, uno posee el archivo y otro entrega una propuesta de parche o contrato, sin escribir.
5. Merges por el coordinador una vez resueltas dependencias. No usar `git reset --hard`, `git clean -fdx`, ni comandos equivalentes sobre trabajo ajeno.
6. En fase 4, analizar fallos en paralelo pero solo **un runner** ejecuta la lista cerrada de pruebas; correcciones en módulos no solapados.
7. En fase 5, indexación paralela por lotes con un commit/journal coordinado e idempotente; bloquear mutaciones concurrentes de un mismo índice.

## Contrato mínimo de respuesta de cada agente

```text
ID / área:
Hechos nuevos (máx. 5):
Evidencia: rutas + símbolos + referencias
Cambios entregados: archivos e IDs
Bloqueos: causa verificable
Estado: COMPLETO | PARCIAL | BLOQUEADO
```

Ni logs íntegros ni reimpresión de documentos originales. Solo fase 1 contiene informe exhaustivo centralizado; los agentes entregan hallazgos estructurados para consolidarlo.

## Prevención de bucles de tokens

- No auto-crear subtareas para enriquecer documentación una vez cumplido el GOAL.
- No reabrir un frente completo por una duda local: buscar evidencia directa y registrar `DESCONOCIDO` cuando no sea verificable.
- Usar inventario de archivos y hashes para evitar relecturas sin cambios; no usar esos hashes para afirmar pruebas de producto.
- Si los recursos permiten solo 4–6 agentes útiles, limitar a ese número y declarar el motivo.
