using System;

namespace Grimoire.Formats;

/// <summary>
/// Direction a message travels in (contract §13 "Nachrichtenkatalog": T = tool, E = engine).
/// </summary>
public enum MessageDirection
{
    /// <summary>
    /// Tool to engine only (<c>T→E</c>).
    /// </summary>
    ToolToEngine,

    /// <summary>
    /// Engine to tool only (<c>E→T</c>).
    /// </summary>
    EngineToTool,

    /// <summary>
    /// Both directions (<c>T↔E</c>).
    /// </summary>
    Both,
}

/// <summary>
/// One row of a generated message catalogue.
/// </summary>
/// <param name="Id">Wire message id.</param>
/// <param name="Name">Message name.</param>
/// <param name="Direction">Direction the message travels in.</param>
/// <param name="PayloadType">Generated payload type.</param>
public sealed record MessageCatalogueEntry(ushort Id, string Name, MessageDirection Direction, Type PayloadType);
