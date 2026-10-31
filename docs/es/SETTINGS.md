# Settings y Hooks de Conexión

Una referencia para cada sección de Settings y para los connection hooks — los
comandos, scripts o snippets de Lua que DBFlux ejecuta alrededor del ciclo de
vida de una conexión.

Abre Settings desde la command palette (**Open settings**) o desde la barra
lateral. La ventana está organizada en secciones a lo largo del lado izquierdo.

| Sección                                             | Cubre                                                                        |
| --------------------------------------------------- | ---------------------------------------------------------------------------- |
| [General](#general)                                 | Comportamiento a nivel de app: theme, inicio, refresh, seguridad de queries. |
| [Audit](#audit)                                     | Qué captura el audit log y cuánto tiempo se conserva.                        |
| [Keybindings](#keybindings)                         | Explora y cambia el keymap.                                                  |
| [Actualizaciones](#actualizaciones)                 | Búsqueda de actualizaciones al iniciar y las novedades tras actualizar.      |
| [Restablecer](#restablecer) | Borra tamaños recordados, disposiciones, historiales, las pestañas restauradas, los ajustes generales o el audit log. |
| [Auth Profiles](#auth-profiles-proxies-ssh-tunnels) | Perfiles AWS SSO / shared-credentials.                                       |
| [Proxies](#auth-profiles-proxies-ssh-tunnels)       | Perfiles de proxy SOCKS5 / HTTP.                                             |
| [SSH Tunnels](#auth-profiles-proxies-ssh-tunnels)   | Perfiles reutilizables de túnel SSH.                                         |
| [Services](#services-rpc)                           | Drivers RPC externos y auth providers.                                       |
| [Hooks](#connection-hooks)                          | Definiciones reutilizables de connection hooks.                              |
| [Drivers](#drivers)                                 | Overrides y ajustes por driver.                                              |
| About                                               | Información de versión y build.                                              |

Las secciones relacionadas con MCP (Clients, Roles, Policies) aparecen solo
cuando el binario se construye con el feature `mcp`, que es el valor por
defecto; ver [AI + MCP
Integration](MCP_AI_INTEGRATION.md).

---

## General

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../images/settings/general-dark.webp">
  <img src="../images/settings/general-light.webp" alt="La ventana de Settings en la sección General, con los ajustes de apariencia, editor y arranque">
</picture>

### Apariencia

| Setting      | Opciones                 | Default |
| ------------ | ------------------------ | ------- |
| **Theme**    | Follow system, Dark, Light | Dark  |
| **Density**  | Default, Compact         | Default |
| **Language** | System y todos los idiomas con un catálogo de traducción incluido | System  |
| **Fuente de la interfaz** | Predeterminada (Archivo) o cualquier fuente instalada | Predeterminada |
| **Tamaño de fuente de la interfaz** | De 8 a 32 px, admite decimales | 13 |
| **Fuente del editor** | Predeterminada (JetBrains Mono) o cualquier fuente instalada | Predeterminada |
| **Tamaño de fuente del editor** | De 8 a 32 px, admite decimales | 13 |
| **Fuente de la tabla de datos** | Igual que el editor o cualquier fuente instalada | Igual que el editor |
| **Tamaño de fuente de la tabla de datos** | De 8 a 32 px, admite decimales | 12.5 |

Los cambios de fuente se aplican a todas las ventanas abiertas en cuanto
guardas; no hace falta reiniciar. La fuente del editor también define el texto
monoespaciado de la interfaz (metadatos, atajos de teclado, consola), y una
fuente de interfaz personalizada también reemplaza la fuente expandida de las
etiquetas de sección. Qué controla cada tamaño:

- **Tamaño de fuente de la interfaz** escala todo el texto, las filas, los
  controles y los íconos de la interfaz.
- **Tamaño de fuente del editor** define el texto y la altura de línea del
  editor de código, y el texto de la consola.
- **Tamaño de fuente de la tabla de datos** define el texto de las celdas, la
  altura de filas y encabezados, y el ancho automático de las columnas. Las
  columnas que redimensionaste a mano conservan su ancho.

Las listas de fuentes tienen un campo de búsqueda, porque un sistema puede tener
cientos de fuentes. Una fuente guardada que ya no está instalada sigue
seleccionada, marcada como "(no instalada)", y DBFlux dibuja con la fuente
predeterminada hasta que se vuelva a instalar; la tabla de datos recurre a la
fuente del editor.

La lista de idiomas se deriva de los catálogos de traducción incluidos con
DBFlux: English aparece primero, seguido de los demás idiomas en un orden
determinista y con sus nombres nativos. System sigue el locale del sistema
operativo y recurre a English cuando ningún locale incluido coincide de forma no
ambigua. Un cambio de idioma tiene efecto después de reiniciar DBFlux, por lo que
el control muestra una nota permanente al respecto. Los catálogos parciales
recurren a English para el texto general aún no traducido. Este release solo
traduce la sección General; el resto de la UI se está convirtiendo crate por crate
y permanece en English por ahora.

### Editor

| Setting                      | Default | Qué hace |
| ---------------------------- | ------- | -------- |
| **Vim mode in editors**      | Off     | Edición modal en todos los editores de varias líneas (editor de código, editor de objetos, diálogos JSON, panel de valor, vista JSON y pipeline de colecciones; solo movimientos en los visores de solo lectura): modos Normal e Insertar con `h`, `j`, `k`, `l`, `i`, `Escape`, `x` y `u`. Se aplica a los editores abiertos al guardar. Ver la sección de [modo Vim](KEYBOARD.md#modo-vim-opcional) en la referencia de teclado. |
| **Leader key**               | Space   | La tecla que inicia las secuencias de líder de Vim en los modos Normal y Visual, como la líder y luego `a` para las acciones del panel: espacio, coma o barra invertida. Se aplica a los editores abiertos al guardar, y los atajos de líder que cambiaste en **Keybindings** la siguen. |
| **Add an alias to table completions** | On | Al aceptar una tabla, vista o CTE del autocompletado SQL después de `FROM` o `JOIN` se inserta con un alias formado por las iniciales de su nombre: `access_control` pasa a ser `access_control ac` y `public.order_items` pasa a ser `public.order_items oi`. Un alias ya usado en la sentencia, una palabra reservada o el nombre de una tabla conocida recibe un número (`ac2`). No se añade alias cuando ya hay uno después del cursor, ni después de `INSERT INTO`. La tabla destino de `UPDATE` y `DELETE FROM` solo recibe alias en las bases de datos que lo aceptan ahí: PostgreSQL y DuckDB, y SQLite y Turso con `AS`. |
| **Write the alias with AS** | Off | Inserta `access_control AS ac` en lugar de `access_control ac`. |

### Cuadrícula de datos

| Setting                      | Default | Qué hace |
| ---------------------------- | ------- | -------- |
| **Mostrar el valor del registro referenciado en celdas de clave foránea** | Off | Las celdas que contienen una clave foránea resuelven y muestran el valor del registro referenciado junto a su id. Se aplica a las páginas cargadas después de guardar; el inspector de filas siempre resuelve las referencias. |
| **Ordenar las tablas por clave primaria descendente** | Off | Las tablas se abren ordenadas por su clave primaria de forma descendente en lugar de ascendente, así con una clave autoincremental las filas más recientes aparecen primero. Al quitar el orden de una columna se vuelve a este orden, y un clic en el encabezado de la columna clave desde este orden la ordena de forma ascendente. Se aplica a las tablas abiertas después de guardar; en una tabla ya abierta, también al quitar el orden de una columna. |
| **Revisar los cambios antes de guardar** | Off | Guardar en la cuadrícula de una tabla muestra primero las sentencias que ejecutará, con Guardar, Copiar SQL y Cancelar, como el botón Revisar. Ver Trabajar con resultados. |
| **Proteger las celdas copiadas contra fórmulas de hojas de cálculo** | Off | Copiar (`Ctrl+C`, `Cmd+C` en macOS) y Copiar como CSV ponen un `'` delante de una celda de texto (y, con nombres de columna, de un nombre) que empieza por `=`, `+`, `-`, `@`, un tabulador o un retorno de carro, para que la hoja de cálculo donde pegas la muestre como texto en lugar de ejecutarla como fórmula. Los números no cambian. Las exportaciones tienen su propia opción en el menú de exportación. Se aplica a las cuadrículas abiertas al guardar. |
| **Incluir los nombres de columna al copiar** | Off | Copiar (`Ctrl+C`, `Cmd+C` en macOS) en la cuadrícula de datos empieza el texto separado por tabulaciones con una línea con los nombres de las columnas copiadas. Copiar como CSV, JSON y las demás entradas Copiar como no cambian. Se aplica a las cuadrículas abiertas al guardar. |
| **Hacer clic en la cabecera de una columna la ordena** | Off | Desactivado: un clic en la cabecera de una columna selecciona la columna entera, y la flecha del borde derecho de la cabecera ordena. Activado: un clic normal ordena la columna (sin orden, ascendente, descendente), y `Shift`+clic y `Ctrl`+clic siguen seleccionando columnas. Las tablas que mantienen el orden del archivo (CSV, Parquet, hojas de cálculo) siempre seleccionan. |
| **Insertar comillas tras una comparación en el filtro WHERE** | Off | Al escribir `=`, `!=`, `<>`, `<=` o `>=` (en su último carácter), `<` o `>` (en el espacio que lo sigue), o `LIKE`, `NOT LIKE`, `ILIKE`, `NOT ILIKE` (en el espacio tras la palabra) después de una columna de texto o de fecha/hora en el filtro `WHERE` de una tabla, se inserta `''` y el cursor queda entre las comillas. Los números, booleanos, `IS NULL`, `IN (...)` y lo que esté dentro de un literal de texto quedan como se escriben, y pegar texto nunca inserta comillas. Ver [Filtrar resultados](RESULTS.md#filtrar-resultados). |

### Inicio y sesión

| Setting                        | Default | Qué hace                                                         |
| ------------------------------ | ------- | ---------------------------------------------------------------- |
| **Restore session on startup** | On      | Reabre las tabs que tenías abiertas la última vez. Las tabs de tablas y colecciones vuelven con el filtro, el límite de filas y el orden con los que se cargaron por última vez; si su conexión aún no está abierta, lo indican y cargan sus filas cuando la conexión está lista (con **Reopen last connections**, o cuando te conectas). Una tab de una conexión que borraste no se reabre, y los resultados de queries abiertos en su propia tab no se restauran. |
| **Reopen last connections**    | Off     | Al iniciar, reconecta las conexiones que estaban abiertas al salir la última vez y las que usan las tabs restauradas. Funciona aunque la sesión no se restaure. Mientras está activado, el interruptor **Conectar al iniciar** de cada conexión no tiene efecto (ver [Conexiones](CONNECTIONS.md)). |
| **Default focus**              | Sidebar | Dónde cae el focus al iniciar (Sidebar o la última tab).         |
| **Max history entries**        | 1000    | Tope del historial de queries (mínimo 10).                       |
| **Auto-save interval (ms)**    | 2000    | Cada cuánto se auto-guarda el contenido del editor (mínimo 500). Un script respaldado por un archivo se escribe en ese archivo; el contenido sin título se conserva en la carpeta `sessions/`. |

### Actualización y segundo plano

| Setting                                 | Default | Qué hace                                                  |
| --------------------------------------- | ------- | --------------------------------------------------------- |
| **Default refresh policy**              | Manual  | Manual o Interval auto-refresh para las data views.       |
| **Default refresh interval (seconds)**  | 5       | Intervalo usado cuando la policy es Interval (mínimo 1).  |
| **Max concurrent background tasks**     | 8       | Tope de las tareas de conexión y de esquema de todas las conexiones (mínimo 1). Cuentan las conexiones, desconexiones, cambios de base de datos, cargas de esquema y de detalles de tabla, actualizaciones y eliminaciones de esquema, y los escaneos key-value en curso. Por encima del tope, una nueva espera en una cola, por orden de llegada, y empieza cuando termina una en curso. Los hooks de una conexión se ejecutan dentro de su hueco y no cuentan aparte. |
| **Max concurrent queries per connection** | 8     | Cuántas queries y comandos se ejecutan a la vez en una conexión (mínimo 1): páginas del data grid, recuentos de filas, detalles de tabla y claves primarias, etiquetas de claves foráneas, ejecuciones del editor SQL, guardados y otras escrituras, exports, gráficos y paneles de dashboard, y búsquedas de filas similares. El resto espera en la cola de esa conexión. Cada conexión tiene su propio límite, así que una conexión ocupada no frena a otra. Una ejecución o un guardado explícitos pasan por delante de las lecturas en segundo plano en cola. |
| **Max queued queries per connection**   | 50      | Cuántas queries pueden esperar en una conexión (mínimo 1). Con la cola llena, una nueva lectura en segundo plano se cancela con un aviso y su pestaña muestra el estado cancelado, así que recargar lo vuelve a intentar; una ejecución o un guardado explícitos ocupan en cambio el lugar de la lectura en segundo plano más antigua, que se cancela con el mismo aviso. Lo que espera aparece como En espera en el panel de tareas, se puede cancelar allí, y **Vaciar cola (N)** (`Shift+c` en el panel, la paleta de comandos o el menú de la conexión) cancela todo lo que espera en una conexión. |
| **Pause auto-refresh on error**         | On      | Detiene el auto-refresh de una view después de que falla. |
| **Auto-refresh only if tab is visible** | Off     | Se salta el refresh de las tabs que no estás mirando.     |

### Notificaciones

| Ajuste | Opciones | Predeterminado |
|---------|---------|---------|
| **Duración de los avisos** | 4 segundos, 8 segundos, 15 segundos, Mantener hasta descartarlos | 8 segundos |

**Duración de los avisos** define cuánto permanece un aviso antes de cerrarse
solo. Los avisos de éxito y de información se cierran al cabo de ese tiempo, y
los de advertencia al doble. Un error, y un aviso con un botón de acción o una
barra de progreso, permanece hasta que lo descartes. **Mantener hasta
descartarlos** desactiva el cierre automático para todos los avisos. Pasar el
puntero por encima de un aviso detiene su temporizador, que se reanuda cuando el
puntero se va.

Los avisos se apilan en la esquina superior derecha del área de documentos. Se
muestran como máximo cuatro a la vez; los más antiguos se pliegan en una entrada
*N más* que los expande o los descarta todos juntos.

### Seguridad de ejecución (confirmación de queries peligrosas)

Estos tres settings gobiernan cómo DBFlux trata las queries riesgosas en
**todos** los drivers y query languages. No hay un toggle por base de datos —
las mismas reglas aplican a `DELETE`/`DROP`/`TRUNCATE` de SQL,
`deleteMany`/`drop` de MongoDB, `FLUSHALL`/`FLUSHDB` de Redis, etc.

| Setting                                          | Default | Qué hace                                                                                                    |
| ------------------------------------------------ | ------- | ----------------------------------------------------------------------------------------------------------- |
| **Confirm dangerous queries**                    | On      | Muestra una confirmación antes de ejecutar una query peligrosa. Desactívalo para permitirlas sin preguntar. |
| **Require WHERE for DELETE/UPDATE**              | On      | Trata un `DELETE`/`UPDATE` sin `WHERE` como peligroso.                                                      |
| **Always require preview (ignore suppressions)** | Off     | Fuerza el modal de confirmación/preview incluso para queries que anteriormente elegiste dejar de confirmar. |

**Confiar en claves de host SSH nuevas sin preguntar** (desactivado, no
recomendado) deja que un túnel SSH conecte a un servidor cuya clave de host no
conocen ni tus archivos known_hosts ni DBFlux y la recuerde, en lugar de pedirte
que compares antes su huella. También cubre las conexiones que no pueden
preguntar, como Conectar al iniciar y el servidor MCP. Una clave cambiada
siempre se rechaza. No se aplica cuando known_hosts o DBFlux ya tienen una clave de
otro tipo para el host, y el servidor MCP solo lo lee al arrancar. Ver
[Claves de host](CONNECTIONS.md#claves-de-host).

**Editor row limit** (límite de filas del editor, 10.000 por defecto) limita
cuántas filas devuelve una query ejecutada desde el editor. Es una opción de
seguridad independiente de los tres controles de queries peligrosas de arriba.
Acepta cualquier número entero desde 1 y no se puede poner en cero ni
desactivar: al guardar cualquier otro valor se muestra un error y se conserva
el límite anterior. DBFlux envía el límite al driver
en lugar de agregar un `LIMIT` al texto de la query. Cuando un resultado omitió
filas, su pie dice "Primeras 10,000 filas cargadas" en lugar del número de
filas. Para una sola sentencia que solo lee, el pie ofrece entonces **Contar
filas**, que ejecuta la sentencia dentro de `SELECT COUNT(*)` sin traer sus
filas y muestra "10,000/52,310 filas cargadas" (solo conexiones SQL), y
**Cargar todas las filas**, que vuelve a ejecutar la sentencia sin el límite en
la misma pestaña de resultados. Ambas están también en el menú de la tabla
(`m`). En un script con varias sentencias, el límite es un único presupuesto
compartido por todos sus resultados, y todas las sentencias se ejecutan igual.

El límite solo llega a un driver cuyo `execute` lo aplica: PostgreSQL,
MySQL/MariaDB, SQL Server, SQLite, DuckDB, MongoDB, Redis, Turso, InfluxDB,
ClickHouse, Redshift, CloudWatch y DynamoDB. Algunos todavía leen la respuesta
completa antes de recortarla, y Redis solo limita las respuestas que se muestran
como tabla; la página de cada driver detalla sus límites. Los drivers RPC
externos reciben el límite cuando DBFlux y el driver hablan driver RPC 1.6 o
posterior y el driver declara `REQUEST_ROW_LIMIT`; cualquier otro driver externo
no recibe ninguno y la query se ejecuta sin límite. Un resultado sin límite muestra su recuento de filas
completo, porque no se omitió ninguna, y su pie no ofrece ni Contar filas ni
Cargar todas las filas; agrega un `LIMIT` o un `.limit()` a la sentencia cuando
el resultado pueda ser grande. El límite no agrega un timeout y no se aplica a
scripts Lua, Python o Bash, hooks de conexión ni métricas. La
[consola](CONSOLE.md) envía el mismo límite a los drivers que pueden aplicarlo y
ejecuta sus comandos sin él en los demás.

### Almacenamiento (solo builds Nightly)

| Setting                     | Default | Qué hace                                                                                                               |
| --------------------------- | ------- | ---------------------------------------------------------------------------------------------------------------------- |
| **Use the stable database** | Off     | Hace que un build Nightly comparta el `dbflux.db` stable en lugar de `dbflux-nightly.db`. Aplica en el próximo inicio. |

Ver [Data & Privacy](PRIVACY.md#ubicaciones-de-datos) para cómo se separan
las bases de datos Nightly y stable.

---

## Audit

La sección Audit controla el audit log unificado. El control principal orientado
al usuario es **Log Capture → Minimum Level** (trace / debug / info / warn /
error), que determina cuánto del logging interno de DBFlux se pliega en el audit
trail. Guardar tiene efecto sin reiniciar.

La retention (cuánto tiempo se conservan los eventos) impulsa un purge periódico
en segundo plano cuando está configurada. Para la experiencia diaria de audit —
abrir el viewer, filtrar, exportar — ver [Audit → Visor de audit](AUDIT.md#visor-de-audit). Para el schema completo de eventos
y el comportamiento de redaction ver [Audit](AUDIT.md) y [Data &
Privacy](PRIVACY.md#auditoría-y-privacidad).

---

## Keybindings

Esta sección lista el keymap activo agrupado por contexto. Fíltralo por comando,
tecla o predicado de contexto con el campo de texto, o muestra un solo contexto
con el filtro de contexto. Un contexto que hereda de otro (el Editor hereda de
Global) también lista los bindings heredados que no sombrea. El keymap por
defecto completo está documentado en [Referencia de teclado](KEYBOARD.md).

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../images/settings/keybindings-dark.webp">
  <img src="../images/settings/keybindings-light.webp" alt="La sección Keybindings de la ventana de Settings, con los atajos globales y un lápiz para cambiar cada uno">
</picture>

**Cambiar un atajo.** Pulsa el lápiz de un binding, o selecciónalo y pulsa
`Enter`, y luego pulsa las teclas nuevas. Un atajo puede ser una tecla con sus
modificadores o una secuencia de hasta cuatro, como `g g` o `Ctrl+K Ctrl+S`:
púlsalas una tras otra, y la grabación se guarda tras una pausa de un segundo.
`Esc` cancela. Mientras grabas, **Quitar atajo** deja el binding sin tecla.
También se pueden grabar teclas que usa la propia ventana de Settings, como
`Ctrl+S` o `Tab`. Un `Esc` solo no se puede grabar porque cancela.

**Contextos.** Cada binding se aplica en un contexto, escrito como un predicado
en el mismo lenguaje que usa Zed. Pulsa el botón de capas de un binding, o
selecciónalo y pulsa `p`, para editarlo; `Enter` guarda y `Esc` cancela. Un
predicado combina nombres con `&&`, `||` y `!`, compara un valor con `==` o
`!=`, y usa `A > B` para "B dentro de A", con paréntesis para agrupar:

| Predicado | Se aplica |
|-----------|-----------|
| `Editor && !Modal` | en un editor de código, no con un diálogo abierto |
| `Editor && vim_mode == normal` | en un editor de código en el modo Normal de Vim |
| `Editor && language == mongo` | en un editor de código de una consulta MongoDB |
| `Results \|\| Audit` | en una grilla de resultados o en el visor de auditoría |
| `SidebarPanel && tab == scripts` | en la pestaña Scripts de la barra lateral |
| `CodeEditor > Input` | en el buffer de texto del editor de código |

Los nombres son los de los contextos que muestra el filtro de contexto
(`Global`, `Sidebar`, `Editor`, `Results`, `DataTable`, `Input`, `Modal`, …),
los nombres de ventana `Workspace`, `SettingsWindow` y
`ConnectionManagerWindow`, y los nombres de panel `ActivityRail`,
`CommandSearch`, `SidebarPanel`, `CodeEditor`, `ResultPanel`, `RowInspector`,
`KeyValueConsole`, `NativeConsole`, `DocumentQueryBar`, `DocumentSchema`, `DocumentAggregate`,
`DashboardsPanel` y
`SettingsSection`. Los valores son `vim_mode` (`normal`, `insert`, `replace`,
`visual`, `visual_line`, `visual_block`, presente solo con la edición Vim
activa), `language` (`sql`, `mongo`, `redis`, `lua`, `python`, `bash`, …),
`tab`, `section` y `focus`. Un predicado que no se puede analizar no se guarda;
uno que nombra algo que DBFlux nunca fija se guarda con un aviso, porque nunca
coincide.

**Qué binding gana.** Un binding de un elemento con foco (un campo de texto, una
data table, un diálogo, el árbol de documentos) gana sobre un binding del panel
que lo rodea, y un binding que cambiaste gana sobre uno por defecto asignado a
las mismas teclas en el mismo lugar. Con la edición Vim activa, un binding que
creaste para un modo de Vim, como `space r` en `Editor && vim_mode == normal`,
se ejecuta antes de que Vim lea la tecla; los bindings por defecto dejan a Vim
sus propias teclas. Un botón, casilla o fila de lista con foco siempre toma
`Enter` y `Space` para sí.

**Avisos y conflictos.** Si las teclas nuevas ya las usa otro comando en un
contexto que puede estar activo al mismo tiempo, todavía no se guarda nada: un
aviso nombra el otro comando y su contexto. **Cancelar** deja todo como estaba.
**Reemplazar** asigna las teclas al binding que estás editando y se las quita al
otro, que queda sin atajo hasta que lo restablezcas. Un binding que choca con
otro muestra una etiqueta "en conflicto con", y el pie cuenta los conflictos.
Dos etiquetas avisan sin bloquear: "comparte una secuencia de teclas" cuando las
teclas de un binding empiezan las de otro (DBFlux espera entonces un segundo
tras el más corto para ver si sigue el más largo), y "captura texto escrito"
cuando una letra sola se asigna donde se escribe texto, como un campo de texto o
el editor de código fuera de los modos Normal y Visual de Vim.

**Restablecer.** Un binding modificado muestra una flecha que restablece sus
teclas y su contexto por defecto (`r` sobre un binding seleccionado hace lo
mismo, y `Delete` quita su atajo). **Restablecer predeterminados** en el pie
descarta todas las modificaciones, como `Shift+R` desde la lista. `c` abre el
filtro de contexto. El pie también muestra cuántos bindings están
modificados.

Los cambios se aplican al instante en todas las ventanas, sin reiniciar. Solo se
guardan tus modificaciones, en la tabla `cfg_keybinding_overrides` de
`dbflux.db`, así que los bindings que no cambiaste siguen los valores por defecto
de futuras versiones. Una modificación cuyo binding por defecto elimina una
versión posterior se ignora.

---

## Actualizaciones

| Ajuste | Por defecto | Qué hace |
|--------|-------------|----------|
| **Buscar una nueva versión en GitHub al iniciar** | Activado | Al iniciar, DBFlux consulta la API pública de releases de GitHub por la versión más reciente de su canal (stable, rc o nightly). Si hay una más nueva, un aviso enlaza la descarga, o indica cómo actualizar una instalación hecha con un gestor de paquetes. **Comprobar ahora** hace la misma comprobación en cualquier momento. La petición solo lleva un `User-Agent` con el nombre de la app y su versión. |
| **Mostrar las novedades después de cada actualización** | Activado | El primer inicio de una versión más nueva abre el registro de cambios de cada release desde la versión que usabas antes. Lee el registro de cambios incluido en el binario, así que no necesita red. **Ver novedades** y **Mostrar la bienvenida de nuevo** abren los dos diálogos en cualquier momento. |

Las compilaciones hechas con `--features no-update-check` (por ejemplo
`cargo build --release --features no-update-check`) no tienen búsqueda de
actualizaciones: nunca contactan con GitHub, ni al iniciar ni desde Settings, y
muestran "Las comprobaciones de actualizaciones están desactivadas en esta
compilación." en lugar de la casilla de inicio y **Comprobar ahora**, aquí y en
el diálogo de bienvenida, diga lo que diga el ajuste guardado. Las novedades
siguen funcionando. El feature está pensado para forks y compilaciones locales,
cuyas versiones no coinciden con las releases publicadas y que de otro modo
avisarían de actualizaciones que no lo son.

---

## Restablecer

La sección Restablecer borra un tipo de estado guardado en `dbflux.db` cada
vez, para que un tamaño de ventana, una disposición o un historial recordado
que estorba pueda irse sin editar la base de datos a mano. Cada fila muestra lo
que guarda ahora y tiene un botón **Restablecer…**, desactivado mientras no hay
nada que borrar.

Ninguna fila toca las conexiones, los auth profiles, los proxies, los túneles
SSH, los parámetros de driver, los secretos del llavero, los hooks, los
servicios RPC, los ajustes de MCP, las queries guardadas ni los filtros de
audit guardados.

| Fila | Qué se borra | Cuándo aplica |
|------|--------------|---------------|
| **Tamaños de ventanas y diálogos** | Tamaño y posición de la ventana principal, tamaño del editor de celdas y si estaba maximizado, dónde se abren los resultados. | El editor de celdas y las nuevas pestañas de query al momento; la ventana principal abre con su tamaño predeterminado en el próximo inicio. |
| **Disposición de columnas** | Columnas ocultas, movidas, fijadas y redimensionadas, por tabla. | Tablas abiertas después. |
| **Historial de filtros WHERE** | Filtros WHERE anteriores de las tablas y queries anteriores de las colecciones de documentos. | Grids abiertos después. |
| **Opciones del menú de exportación** | Filas, columnas y encabezado CSV del menú de exportación. | El próximo menú de exportación. |
| **Pestañas restauradas** | Las pestañas que se reabren al iniciar y las conexiones que se reconectan con ellas. Las queries borrador sin guardar en ellas se pierden. | El próximo inicio abre sin las pestañas actuales; las abiertas antes de reiniciar tampoco se recuerdan. |
| **Historial de consultas** | El historial del editor SQL. Se conservan las entradas con estrella y las queries guardadas. | Al momento. |
| **Archivos recientes** | La lista de archivos recientes (no los archivos). | Al momento. |
| **Instantáneas de esquema** | Las instantáneas tomadas al conectar para la diferencia de esquemas. | Al momento. |
| **Ajustes generales** | Todos los ajustes de [General](#general) a su valor predeterminado. | Al momento; un cambio de idioma tras reiniciar. |
| **Todo lo anterior** | Todas las filas anteriores de una vez. | Como cada fila. |
| **Registro de auditoría** | Todos los eventos de audit. No forma parte de **Todo lo anterior**. | Al momento. |

**Restablecer…** abre una confirmación que dice qué se borra, cuánto hay
guardado y que no se puede deshacer; el botón rojo restablece y Cancelar (o
Escape) lo conserva todo. **Todo lo anterior** y **Registro de auditoría**
preguntan una segunda vez antes de borrar. Un toast indica cuántas entradas se
eliminaron. Con el audit activado, cada restablecimiento queda en el audit log, así que
tras borrarlo su primera entrada es ese restablecimiento.

`j` y `k` (o las flechas) recorren las filas y `Enter` abre la confirmación de
la fila bajo el cursor.

---

## Auth Profiles, Proxies, Túneles SSH

Estas tres secciones gestionan los perfiles reutilizables que luego seleccionas
por conexión en la pestaña Access. Están documentadas en detalle — campos, flujo
AWS SSO, reglas de no-proxy, métodos de auth SSH — en [Connecting to a Database
→ Advanced Setup](CONNECTIONS.md):

- [Auth Profiles](CONNECTIONS.md#auth-profiles-aws-sso-and-shared-credentials)
- [Proxies](CONNECTIONS.md#proxies)
- [SSH Tunnels](CONNECTIONS.md#ssh-tunnels)

Las credenciales que ingresas aquí se guardan en el keyring de tu sistema
operativo, no en la base de datos. Ver [Data & Privacy →
Secrets](PRIVACY.md#secretos-y-el-keyring-del-sistema-operativo).

---

## Servicios (RPC)

Los drivers externos y los auth providers corren como procesos separados con los
que DBFlux se comunica a través de un socket local. Cada service que agregas
aquí tiene:

| Campo                     | Notas                                                                                                     |
| ------------------------- | --------------------------------------------------------------------------------------------------------- |
| **Socket ID**             | Identificador único, usado como nombre del archivo del socket. Solo letras ASCII, dígitos, `.`, `_`, `-`. |
| **Command**               | El ejecutable a lanzar (opcional para algunas configuraciones).                                           |
| **Startup Timeout (ms)**  | Cuánto esperar a que el proceso arranque. Default 5000.                                                   |
| **Service Type**          | **Driver** o **Auth Provider**.                                                                           |
| **Enable this service**   | Si el service arranca. Default on.                                                                        |
| **Arguments**             | Argumentos ordenados del proceso.                                                                         |
| **Environment Variables** | Pares `KEY=value` pasados al proceso.                                                                     |

Los cambios aquí **tienen efecto en el próximo inicio**. Referencia completa:
[RPC Services Config](RPC_SERVICES_CONFIG.md) y el [Driver RPC
Protocol](DRIVER_RPC_PROTOCOL.md).

---

## Drivers

Elige un driver para ver y sobrescribir su comportamiento. Dos grupos son
editables:

**Global overrides** — versiones por driver de los settings de General. Cada uno
es un tri-state (Inherit / On / Off, o un valor explícito); dejarlo en *Inherit*
usa el default de General mostrado junto al control:

- Refresh policy e interval
- Confirm dangerous queries
- Require WHERE
- Require preview

**Driver settings** — opciones definidas por el propio driver (renderizadas de
forma genérica a partir del schema del driver, así que los campos disponibles
dependen del driver).

La sección también muestra, en solo lectura, la **capability matrix**, la
category, y el query language del driver.

---

## Connection Hooks

Los hooks son comandos, scripts o snippets de Lua reutilizables que corren
alrededor del ciclo de vida de una conexión. Los **defines** globalmente en
**Settings → Hooks**, y luego los **vinculas** a fases en conexiones
individuales en la pestaña **Hooks** del Connection Manager.

### Camino rápido

1. **Settings → Hooks → agrega un hook.** Dale un **Hook ID**, elige un
   **Type**, y completa el command/script.
2. Abre una conexión en **Connection Manager → Hooks tab**.
3. Selecciona tu hook en uno de los cuatro dropdowns de fase (Pre-connect,
   Post-connect, Pre-disconnect, Post-disconnect).
4. Conecta. La salida del hook se transmite al panel de **Tasks**.

### Tipos de Hook

| Type        | Qué ejecuta              | Qué proporcionas                                                                                                                         |
| ----------- | ------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------- |
| **Command** | Un ejecutable            | Un comando y argumentos separados por espacios.                                                                                          |
| **Script**  | Un archivo Bash o Python | Un lenguaje, una ruta de archivo, y un override opcional del interpreter (en blanco = `bash` / `python3`, ajustado según la plataforma). |
| **Lua**     | Un script Lua in-process | Una ruta de archivo y un conjunto de capabilities (ver abajo). Lua corre dentro de DBFlux — sin interpreter externo.                     |

Los scripts se editan en el editor de DBFlux y se guardan por defecto bajo una
carpeta `hooks/`.

#### Capacidades de Lua

Un hook de Lua solo obtiene las habilidades que habilitas:

| Capability                 | Default | Otorga                                                            |
| -------------------------- | ------- | ----------------------------------------------------------------- |
| **Logging**                | On      | Escribir en la salida del hook.                                   |
| **Environment read**       | On      | Leer variables de entorno.                                        |
| **Connection metadata**    | On      | Leer la metadata del perfil que se está conectando.               |
| **Controlled process run** | Off     | Llamar a `dbflux.process.run(...)` para lanzar procesos externos. El proceso recibe solo una lista corta de las variables de entorno de DBFlux (como `PATH`, `HOME` y el locale); cualquier otra, como credenciales de AWS o `SSH_AUTH_SOCK`, se pasa en su opción `env`. |

> Habilitar **Controlled process run** permite que el hook ejecute comandos
> externos arbitrarios. DBFlux muestra una advertencia de seguridad cuando está
> activado, tanto en la definición del hook como en el binding por conexión.
> Habilítalo solo para hooks en los que confíes.

El runtime de Lua embebido (APIs disponibles, sandboxing) está documentado en
[Lua Scripting](LUA.md).

### Opciones de Hook

| Option                         | Notas                                                                                                       |
| ------------------------------ | ----------------------------------------------------------------------------------------------------------- |
| **Enabled**                    | Los hooks deshabilitados se omiten.                                                                         |
| **Working Directory**          | El cwd del process/script (no usado por Lua).                                                               |
| **Environment**                | Pares extra `KEY=value`.                                                                                    |
| **Inherit parent environment** | On por defecto; pasa el env de DBFlux al hook.                                                              |
| **Env Denylist**               | Nombres de variables a quitar del env heredado.                                                             |
| **Timeout (ms)**               | En blanco = sin timeout. Al hacer timeout se mata el process group.                                         |
| **Execution mode**             | **Blocking** (default) espera al hook; **Detached** corre en segundo plano y no bloquea connect/disconnect. |
| **Ready signal** (Detached)    | Texto que DBFlux espera en la salida del hook antes de continuar.                                           |
| **On Failure**                 | La failure policy — ver abajo.                                                                              |

DBFlux siempre inyecta variables de entorno de contexto en los hooks de proceso:
`DBFLUX_PROFILE_ID`, `DBFLUX_PROFILE_NAME`, `DBFLUX_DB_KIND`, y, cuando se
conocen, `DBFLUX_HOST`, `DBFLUX_PORT`, `DBFLUX_DATABASE`.

> **Los secrets nunca se filtran accidentalmente a los hooks.** Además de tu Env
> Denylist, DBFlux siempre quita las variables heredadas cuyo nombre contiene
> `SECRET`, `TOKEN`, `PASSWORD`, o `_KEY`, y cualquier variable `AWS_*`.

### Políticas de fallo

Qué pasa cuando un hook falla (exit distinto de cero, timeout, o error):

| Policy                   | Efecto                                                        |
| ------------------------ | ------------------------------------------------------------- |
| **Disconnect** (default) | Aborta la fase — el flujo de connect o disconnect se detiene. |
| **Warn**                 | Continúa, pero muestra una advertencia.                       |
| **Ignore**               | Continúa; el fallo solo se registra en el log.                |

### Fases

| Phase               | Corre                             |
| ------------------- | --------------------------------- |
| **Pre-connect**     | Antes de que la conexión se abra. |
| **Post-connect**    | Después de un connect exitoso.    |
| **Pre-disconnect**  | Antes de desconectar.             |
| **Post-disconnect** | Después de desconectar.           |

La pestaña Hooks de una conexión tiene un dropdown por fase (más un input
"Extra" para vincular hook IDs adicionales). Los dropdowns listan los hooks
reutilizables que definiste en Settings → Hooks. Cada hook corre como su propia
background task con stdout/stderr en vivo en el panel Tasks; la salida tiene un
tope de 4 MiB por hook.

---

## Relacionado

- [Primeros pasos](GETTING_STARTED.md) — flujo principal.
- [Referencia de teclado](KEYBOARD.md) — el keymap por defecto completo.
- [Connecting → Advanced Setup](CONNECTIONS.md) — SSH, proxy, auth, fuentes de
  valores.
- [Data & Privacy](PRIVACY.md#tus-datos-en-este-equipo) — dónde se almacenan los settings y
  secrets.
- [Lua Scripting](LUA.md) — el runtime de Lua embebido para hooks.
